#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Exercise workflow policy scripts against real Git changes and job outcomes."""

from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import tempfile
import textwrap
import unittest


ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = {
    "portable": ("portable.yml", "portable-gate", "run_matrix"),
    "worker": ("ortools-worker.yml", "worker-gate", "run_workers"),
}


def run_block(filename: str, job: str) -> str:
    """Read the first literal run block in the named, single-script policy job."""
    lines = (ROOT / ".github/workflows" / filename).read_text().splitlines()
    start = lines.index(f"  {job}:") + 1
    end = next(
        (index for index in range(start, len(lines))
         if lines[index].startswith("  ") and not lines[index].startswith("   ")
         and lines[index].strip()),
        len(lines),
    )
    start = lines.index("        run: |", start, end) + 1
    end = next(
        (index for index in range(start, end)
         if lines[index].strip() and not lines[index].startswith("          ")),
        end,
    )
    return textwrap.dedent("\n".join(lines[start:end]))


def isolated_environment(directory: Path) -> dict[str, str]:
    environment = {
        key: value for key, value in os.environ.items()
        if not key.startswith("GIT_")
        and key not in {"BASH_ENV", "ENV", "SHELLOPTS", "BASHOPTS", "CDPATH"}
    }
    environment.update({
        "GIT_CONFIG_NOSYSTEM": "1",
        "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_ATTR_NOSYSTEM": "1",
        "GIT_AUTHOR_NAME": "CI policy fixture",
        "GIT_AUTHOR_EMAIL": "ci-policy@example.invalid",
        "GIT_COMMITTER_NAME": "CI policy fixture",
        "GIT_COMMITTER_EMAIL": "ci-policy@example.invalid",
        "GIT_AUTHOR_DATE": "2000-01-01T00:00:00Z",
        "GIT_COMMITTER_DATE": "2000-01-01T00:00:00Z",
        "GITHUB_OUTPUT": str(directory / "output"),
        "GITHUB_STEP_SUMMARY": str(directory / "summary"),
    })
    return environment


def execute(script: str, directory: Path, environment: dict[str, str]):
    return subprocess.run(
        ["bash", "--noprofile", "--norc", "-eo", "pipefail", "-c", script],
        cwd=directory, env=environment, text=True, capture_output=True, check=False,
    )


class CiPolicyTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.selectors = {
            name: run_block(filename, "select-targets")
            for name, (filename, _, _) in WORKFLOWS.items()
        }
        cls.gates = {
            name: run_block(filename, gate)
            for name, (filename, gate, _) in WORKFLOWS.items()
        }

    def selections(self, paths=(), *, base_ref="phase/06-desktop",
                   event="pull_request", rename=False):
        with tempfile.TemporaryDirectory(prefix="eutheto-ci-policy-") as temporary:
            directory = Path(temporary)
            environment = isolated_environment(directory)
            hooks = directory / "empty-hooks"
            hooks.mkdir()

            def git(*arguments):
                return subprocess.run(
                    ["git", "-c", f"core.hooksPath={hooks}", "-c", "commit.gpgsign=false",
                     "-c", "core.autocrlf=false", *arguments],
                    cwd=directory, env=environment, text=True, capture_output=True,
                    check=True,
                ).stdout.strip()

            git("init", "-q", "-b", "main", f"--template={hooks}")
            native = directory / "crates/probe/src/lib.rs"
            native.parent.mkdir(parents=True)
            native.write_text("pub fn native_contract() {}\n")
            git("add", ".")
            git("commit", "-qm", "Initial fixture")
            base = git("rev-parse", "HEAD")
            for path in paths:
                destination = directory / path
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_text("changed fixture\n")
            if rename:
                destination = directory / "apps/desktop/src/moved.ts"
                destination.parent.mkdir(parents=True, exist_ok=True)
                native.rename(destination)
            if paths or rename:
                git("add", "-A")
                git("commit", "-qm", "Policy fixture change")
            environment.update(BASE_SHA=base, BASE_REF=base_ref, EVENT_NAME=event)
            results = {}
            for name, script in self.selectors.items():
                Path(environment["GITHUB_OUTPUT"]).write_text("")
                result = execute(script, directory, environment)
                self.assertEqual(result.returncode, 0, result.stderr)
                outputs = dict(line.split("=", 1) for line in
                               Path(environment["GITHUB_OUTPUT"]).read_text().splitlines())
                flag = WORKFLOWS[name][2]
                results[name] = (
                    outputs[flag],
                    {target["os"] for target in json.loads(outputs["matrix"])["include"]},
                )
            return results

    def assert_selection(self, results, *, run, intel):
        for name, (flag, platforms) in results.items():
            with self.subTest(workflow=name):
                self.assertEqual(flag, "true" if run else "false")
                self.assertEqual("macos-15-intel" in platforms, intel)

    def test_phase_frontend_and_documentation_defer_native_work(self):
        self.assert_selection(self.selections((
            "apps/desktop/src/main.ts", "apps/desktop/index.html",
            "apps/desktop/public/logo.svg", "docs/contributors/example.md",
        )), run=False, intel=False)

    def test_phase_unknown_native_api_and_mixed_changes_require_full_work(self):
        for paths in (
            ("apps/desktop/src/api/client.ts",),
            ("apps/desktop/src-tauri/src/lib.rs",),
            ("new-unknown-input",),
            ("docs/example.md", "Cargo.toml"),
            (".github/workflows/pr.yml",),
            ("crates/probe/README.md",),
            (),
        ):
            with self.subTest(paths=paths):
                self.assert_selection(self.selections(paths), run=True, intel=True)

    def test_native_to_frontend_rename_does_not_hide_deleted_native_input(self):
        self.assert_selection(self.selections(rename=True), run=True, intel=True)

    def test_newline_in_path_cannot_be_misclassified_as_two_allowed_paths(self):
        self.assert_selection(self.selections((
            "README.md\ndocs/apparent.md",
        )), run=True, intel=True)

    def test_main_and_similar_branch_keep_existing_path_policy(self):
        for base_ref in ("main", "phase/06-desktop-extra"):
            with self.subTest(base_ref=base_ref):
                self.assert_selection(self.selections(
                    ("apps/desktop/src/main.ts",), base_ref=base_ref,
                ), run=True, intel=False)
        self.assert_selection(self.selections(
            ("docs/example.md",), base_ref="main",
        ), run=False, intel=False)
        self.assert_selection(self.selections(
            ("apps/desktop/src-tauri/src/lib.rs",), base_ref="main",
        ), run=True, intel=True)

    def test_non_pr_events_always_select_full_approved_matrices(self):
        for event in ("workflow_dispatch", "merge_group", "push", "schedule"):
            with self.subTest(event=event):
                self.assert_selection(self.selections(event=event), run=True, intel=True)

    def gate(self, name, *, selection="true", plan="success", result="success",
             windows=None):
        with tempfile.TemporaryDirectory(prefix="eutheto-ci-gate-") as temporary:
            directory = Path(temporary)
            environment = isolated_environment(directory)
            environment.update({
                "PLAN_RESULT": plan,
                "RUN_MATRIX": selection,
                "RUN_WORKERS": selection,
                "MATRIX_RESULT": result,
                "UNIX_RESULT": result,
                "WINDOWS_RESULT": result if windows is None else windows,
            })
            return execute(self.gates[name], directory, environment).returncode

    def test_gates_accept_only_successful_selection_or_explicit_deferral(self):
        for name in WORKFLOWS:
            with self.subTest(workflow=name):
                self.assertEqual(self.gate(name), 0)
                self.assertEqual(self.gate(name, selection="false", result="skipped"), 0)
                self.assertNotEqual(self.gate(name, selection="false"), 0)

    def test_gates_reject_missing_unknown_or_failed_selection(self):
        for name in WORKFLOWS:
            for selection in ("", "unknown"):
                with self.subTest(workflow=name, selection=selection):
                    self.assertNotEqual(self.gate(
                        name, selection=selection, result="skipped",
                    ), 0)
            with self.subTest(workflow=name, plan="failure"):
                self.assertNotEqual(self.gate(name, plan="failure"), 0)

    def test_selected_failed_cancelled_or_skipped_jobs_cannot_pass(self):
        for result in ("failure", "cancelled", "skipped"):
            for name in WORKFLOWS:
                with self.subTest(workflow=name, result=result):
                    self.assertNotEqual(self.gate(name, result=result), 0)
            with self.subTest(workflow="worker", windows=result):
                self.assertNotEqual(self.gate("worker", windows=result), 0)


if __name__ == "__main__":
    unittest.main()
