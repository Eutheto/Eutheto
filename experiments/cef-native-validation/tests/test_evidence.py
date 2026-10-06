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
        for extra in ({"private": "PRIVATE_SENTINEL"}, {"events": ["PRIVATE_SENTINEL"]}, {"pdfBytes": True}):
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


if __name__ == "__main__":
    unittest.main()
