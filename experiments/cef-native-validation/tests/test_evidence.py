import contextlib
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("probe", Path(__file__).resolve().parents[1] / "probe.py")
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)


class EvidencePrivacy(unittest.TestCase):
    def test_summary_refuses_nested_private_data_at_both_public_sinks(self):
        for extra in ({"private": "PRIVATE_SENTINEL"}, {"events": ["PRIVATE_SENTINEL"]}, {"pdfBytes": True}, {"boundaryStage": "PRIVATE_SENTINEL"}, {"observationFailure": {"code": "other", "path": "PRIVATE_SENTINEL"}}, {"cleanupObservationFailure": {"code": "PRIVATE_SENTINEL"}}, {"nativeLogCategories": ["PRIVATE_SENTINEL"]}, {"action": "shutdown-diagnostic"}, {"action": "shutdown-diagnostic", "passed": False}):
            with self.subTest(extra=extra), tempfile.TemporaryDirectory() as directory:
                source = Path(directory) / "evidence.json"
                summary = Path(directory) / "summary.md"
                source.write_text(json.dumps({"phase": "complete", "passed": True, "cases": [{"mode": "normal", "action": None, "passed": True, **extra}]}))
                output = io.StringIO()
                with patch.dict(os.environ, {"GITHUB_STEP_SUMMARY": str(summary)}), contextlib.redirect_stdout(output):
                    probe.summary(source)
                self.assertEqual(json.loads(output.getvalue()), {"phase": "missing-evidence", "passed": False})
                self.assertNotIn("PRIVATE_SENTINEL", summary.read_text())
                self.assertIn('"passed": false', summary.read_text())

    def test_tool_diagnostics_discard_private_text_and_bound_codes(self):
        raw = b"/PRIVATE_SENTINEL/main.rs:123: error[E0432]: unresolved import PRIVATE_SENTINEL\n"
        raw += b"/PRIVATE_SENTINEL/secret.rs:456: PRIVATE_SENTINEL\n"
        raw += b"error[E0599] " * 20
        facts = probe.tool_failure_facts(raw)
        self.assertEqual(facts["locations"], [{"file": "main.rs", "line": 123}])
        self.assertEqual(facts["codes"], [{"family": "E", "number": 432}] + [{"family": "E", "number": 599}] * 11)
        self.assertEqual(facts["categories"], ["rust-import"])
        self.assertNotIn("PRIVATE_SENTINEL", json.dumps(facts))
        linker = probe.tool_failure_facts(b"LINK : fatal error LNK1104: cannot open file 'PRIVATE_SENTINEL.lib'")
        self.assertEqual(linker, {"locations": [], "codes": [{"family": "LNK", "number": 1104}], "categories": ["msvc-cannot-open"]})

    def test_observation_diagnostics_only_publish_closed_values(self):
        error = RuntimeError("PRIVATE_SENTINEL")
        error.operation = "identity"
        error.native_error = "permission"
        error.native_result = "PRIVATE_SENTINEL"
        self.assertEqual(probe.observation_facts(error), {"code": "other", "operation": "identity", "error": "permission"})


if __name__ == "__main__":
    unittest.main()
