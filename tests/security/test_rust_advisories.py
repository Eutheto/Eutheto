#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Exercise the actual advisory gate's expiry and negative scope boundaries."""

from copy import deepcopy
from datetime import date
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[2]
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("advisory_gate", ROOT / "scripts/check_rust_advisories.py")
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


class AdvisoryGateTests(unittest.TestCase):
    def setUp(self):
        self.ignored = set(gate.MAINTENANCE) | gate.LEGACY_IGNORES
        packages = [dict(name=name, version=version, source=gate.SOURCE, checksum=checksum)
                    for name, version, checksum in gate.MAINTENANCE.values()]
        self.lock = {"package": packages}
        rows = [dict(kind="unmaintained", package=deepcopy(package), advisory=dict(
                    id=advisory_id, package=package["name"], informational="unmaintained"))
                for advisory_id, package in zip(gate.MAINTENANCE, packages)]
        self.report = {"settings": {"ignore": [], "target_arch": [], "target_os": [],
                                    "severity": None,
                                    "informational_warnings": ["unmaintained", "unsound", "notice"]},
                       "database": {"advisory-count": 100},
                       "vulnerabilities": {"found": False, "count": 0, "list": []},
                       "warnings": {"unmaintained": rows}}

    def test_inclusive_utc_deadline(self):
        for today in (date(2026, 10, 18), date(2026, 11, 16)):
            gate.check_scope(self.lock, self.ignored, today)
        with self.assertRaisesRegex(ValueError, "expired"):
            gate.check_scope(self.lock, self.ignored, date(2026, 11, 17))

    def test_every_exact_package_identity_is_required(self):
        for index in range(6):
            for field in ("name", "version", "source", "checksum"):
                with self.subTest(index=index, field=field):
                    lock = deepcopy(self.lock)
                    lock["package"][index][field] += "-changed"
                    with self.assertRaisesRegex(ValueError, "re-review"):
                        gate.check_scope(lock, self.ignored, date(2026, 11, 16))
        self.lock["package"].append(deepcopy(self.lock["package"][0]))
        with self.assertRaises(ValueError):
            gate.check_scope(self.lock, self.ignored, date(2026, 11, 16))

    def test_only_six_matching_maintenance_notices_pass(self):
        self.assertTrue(gate.check_report(self.report, self.ignored))
        for field in ("name", "version", "source", "checksum"):
            report = deepcopy(self.report)
            report["warnings"]["unmaintained"][0]["package"][field] += "-changed"
            with self.subTest(field=field), self.assertRaises(ValueError):
                gate.check_report(report, self.ignored)

    def test_maintenance_id_cannot_become_a_vulnerability(self):
        row = self.report["warnings"]["unmaintained"].pop()
        self.report["vulnerabilities"] = {"found": True, "count": 1, "list": [row]}
        with self.assertRaisesRegex(ValueError, "classification"):
            gate.check_report(self.report, self.ignored)

    def test_maintenance_classification_must_remain_unmaintained(self):
        for classification in (None, "unsound", "notice"):
            report = deepcopy(self.report)
            report["warnings"]["unmaintained"][0]["advisory"]["informational"] = classification
            with self.subTest(classification=classification), self.assertRaises(ValueError):
                gate.check_report(report, self.ignored)

    def test_five_protected_ids_cannot_be_ignored_or_accepted(self):
        for advisory_id in gate.PROTECTED:
            with self.subTest(advisory_id=advisory_id):
                ignored = self.ignored | {advisory_id}
                with self.assertRaises(ValueError):
                    gate.check_scope(self.lock, ignored, date(2026, 11, 16))
                report = deepcopy(self.report)
                report["vulnerabilities"] = {"found": True, "count": 1, "list": [
                    {"advisory": {"id": advisory_id}, "package": {"name": "zip"}}]}
                with self.assertRaisesRegex(ValueError, "forbidden"):
                    gate.check_report(report, self.ignored)

    def test_new_advisory_and_non_advisory_warning_fail(self):
        for row in ({"advisory": {"id": "RUSTSEC-2099-0001"}},
                    {"package": {"name": "yanked-package"}}):
            report = deepcopy(self.report)
            report["warnings"]["notice"] = [row]
            with self.assertRaisesRegex(ValueError, "unreviewed"):
                gate.check_report(report, self.ignored)

    def test_an_extra_ignored_id_cannot_suppress_a_new_vulnerability(self):
        advisory_id = "RUSTSEC-2099-0001"
        ignored = self.ignored | {advisory_id}
        report = deepcopy(self.report)
        report["vulnerabilities"] = {"found": True, "count": 1, "list": [
            {"advisory": {"id": advisory_id}, "package": {"name": "new-vulnerable-dependency"}}]}
        with self.assertRaisesRegex(ValueError, "existing GTK policy"):
            gate.check_scope(self.lock, ignored, date(2026, 11, 16))
        with self.assertRaisesRegex(ValueError, "existing GTK policy"):
            gate.check_report(report, ignored)

    def test_missing_duplicate_or_prefiltered_findings_fail(self):
        for mutation in ("missing", "duplicate", "filtered", "empty-database", "error"):
            report = deepcopy(self.report)
            rows = report["warnings"]["unmaintained"]
            if mutation == "missing":
                rows.pop()
            elif mutation == "duplicate":
                rows.append(deepcopy(rows[0]))
            elif mutation == "filtered":
                report["settings"]["ignore"] = [next(iter(gate.MAINTENANCE))]
            elif mutation == "empty-database":
                report["database"]["advisory-count"] = 0
            else:
                report["error"] = "database unavailable"
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                gate.check_report(report, self.ignored)

    def test_existing_gtk_exception_still_passes(self):
        self.report["vulnerabilities"] = {"found": True, "count": 1, "list": [
            {"advisory": {"id": "RUSTSEC-2024-0429"}}]}
        gate.check_report(self.report, self.ignored)

    def test_architecture_severity_and_warning_filters_fail(self):
        for key, value in (("target_arch", ["x86_64"]), ("target_os", ["linux"]),
                           ("severity", "high"), ("informational_warnings", ["unmaintained"])):
            report = deepcopy(self.report)
            report["settings"][key] = value
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, "unfiltered"):
                gate.check_report(report, self.ignored)

    def test_main_fails_closed_for_audit_infrastructure_errors(self):
        for code, output in ((2, json.dumps(self.report)), (1, ""),
                             (1, '{"error": "fetch failed"}'), (0, "{}")):
            result = subprocess.CompletedProcess([], code, output)
            with self.subTest(code=code, output=output), \
                    patch.object(gate, "check_scope"), \
                    patch.object(gate.subprocess, "run", return_value=result), \
                    patch("sys.stderr", new_callable=io.StringIO):
                self.assertEqual(gate.main(), 1)

    def test_standard_local_and_ci_gates_are_wired(self):
        justfile = (ROOT / "Justfile").read_text()
        recipe = justfile.split("\nrust-advisories:\n", 1)[1].split("\n\n", 1)[0]
        self.assertLess(recipe.index("2026-11-30"), recipe.index("check_rust_advisories.py"))
        self.assertLess(recipe.index("check_rust_advisories.py"), recipe.index("cargo deny --locked"))
        workflow = (ROOT / ".github/workflows/security.yml").read_text()
        self.assertIn("--command just rust-advisories-test", workflow)
        self.assertIn("--command just rust-advisories\n", workflow)


if __name__ == "__main__":
    unittest.main()
