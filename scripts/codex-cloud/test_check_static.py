# SPDX-License-Identifier: Apache-2.0
"""Focused static checks using copies of the project's contributor inputs."""
from contextlib import redirect_stderr, redirect_stdout
import io
import json
from pathlib import Path
import shutil
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
import check_static


class StaticChecks(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="eutheto-static-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for directory in (".codex/agents", ".agents/skills", ".agents/vendor/ponytail"):
            shutil.copytree(check_static.ROOT / directory, self.root / directory)
        manifest = self.root / check_static.MANIFEST
        manifest.parent.mkdir(parents=True)
        shutil.copyfile(check_static.ROOT / check_static.MANIFEST, manifest)

    def replace_role(self, old, new, role="implementation"):
        path = self.root / f".codex/agents/{role}.toml"
        text = path.read_text()
        self.assertIn(old, text)
        path.write_text(text.replace(old, new, 1))

    def test_project_inputs_pass_without_changes(self):
        before = {path.relative_to(self.root): path.read_bytes()
                  for path in self.root.rglob("*") if path.is_file()}
        self.assertEqual(check_static.validate(self.root), [])
        after = {path.relative_to(self.root): path.read_bytes()
                 for path in self.root.rglob("*") if path.is_file()}
        self.assertEqual(before, after)

    def test_missing_extra_and_malformed_roles_fail(self):
        path = self.root / ".codex/agents/implementation.toml"
        original = path.read_text()
        path.unlink()
        self.assertTrue(check_static.validate(self.root))
        path.write_text("name = [")
        self.assertTrue(check_static.validate(self.root))
        path.write_text(original)
        path.with_name("extra.toml").write_text(original)
        self.assertTrue(check_static.validate(self.root))

    def test_wrong_role_settings_and_empty_fields_fail(self):
        path = self.root / ".codex/agents/implementation.toml"
        original = path.read_text()
        cases = (("name = \"implementation\"", "name = \"other\""),
                 ("model = \"gpt-6.1-sol\"", "model = \"gpt-6-astra\""),
                 ("model_reasoning_effort = \"high\"", "model_reasoning_effort = true"),
                 ("description = \"Implement the agreed scope and validate the resulting behavior.\"",
                  "description = \"  \""))
        for old, new in cases:
            with self.subTest(new=new):
                path.write_text(original)
                self.replace_role(old, new)
                self.assertTrue(check_static.validate(self.root))
        path.write_text(original.split("developer_instructions =", 1)[0]
                        + 'developer_instructions = ""\n')
        self.assertTrue(check_static.validate(self.root))

    def test_removed_cap_security_and_handoff_fail(self):
        for directive in check_static.POLICY:
            with self.subTest(directive=directive):
                path = self.root / ".codex/agents/implementation.toml"
                original = path.read_text()
                self.replace_role(directive, "")
                self.assertTrue(check_static.validate(self.root))
                path.write_text(original)
        self.replace_role("gpt-6.1-sol/high implementation worker", "worker", "planning")
        self.assertTrue(check_static.validate(self.root))

    def test_each_pinned_input_missing_or_changed_fails(self):
        for relative in check_static.INPUTS:
            with self.subTest(path=relative):
                path = self.root / relative
                original = path.read_bytes()
                path.unlink()
                self.assertTrue(check_static.validate(self.root))
                path.write_bytes(original + b"\nchanged\n")
                self.assertTrue(check_static.validate(self.root))
                path.write_bytes(original)

    def test_extra_input_and_symlink_fail(self):
        path = self.root / ".agents/skills/extra/SKILL.md"
        path.parent.mkdir()
        path.write_text("extra")
        self.assertTrue(check_static.validate(self.root))
        path.unlink()
        pinned = self.root / ".agents/skills/ponytail/SKILL.md"
        target = self.root / "copied-skill.md"
        target.write_bytes(pinned.read_bytes())
        pinned.unlink()
        pinned.symlink_to(target)
        self.assertTrue(check_static.validate(self.root))

    def test_invalid_manifest_fails(self):
        path = self.root / check_static.MANIFEST
        original = json.loads(path.read_text())
        cases = ["{", "[]", '{"version":"5.3.2","version":"5.3.2"}']
        for field, value in (("version", "other"), ("commit", "0" * 40),
                             ("sha256", {}), ("sha256", [])):
            cases.append(json.dumps({**original, field: value}))
        hashes = dict(original["sha256"])
        hashes[next(iter(hashes))] = "not-a-digest"
        cases.append(json.dumps({**original, "sha256": hashes}))
        for text in cases:
            with self.subTest(text=text[:80]):
                path.write_text(text)
                self.assertTrue(check_static.validate(self.root))
        path.unlink()
        self.assertTrue(check_static.validate(self.root))

    def test_cli_status_and_scope_on_success_and_failure(self):
        for failing in (False, True):
            if failing:
                (self.root / ".codex/agents/planning.toml").unlink()
            output, errors = io.StringIO(), io.StringIO()
            with redirect_stdout(output), redirect_stderr(errors):
                code = check_static.main([str(self.root)])
            self.assertEqual(code, int(failing))
            self.assertIn("Static repository validation: " + ("FAIL" if failing else "PASS"),
                          output.getvalue())
            self.assertIn("Nested CLI diagnostics: not run", output.getvalue())
            self.assertIn("Hosted delegation admission/completion: not checked", output.getvalue())
            self.assertIn("Device/runtime checks: not checked", output.getvalue())
            self.assertEqual(bool(errors.getvalue()), failing)


if __name__ == "__main__":
    unittest.main()
