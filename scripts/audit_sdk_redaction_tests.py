#!/usr/bin/env python3
"""Regression tests for machine-generated SDK redaction evidence."""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
AUDIT = ROOT / "scripts" / "audit_sdk_redaction.py"


class RedactionEvidenceTests(unittest.TestCase):
    def test_automated_evidence_does_not_claim_human_review(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "redaction.json"
            result = subprocess.run(
                [
                    sys.executable,
                    str(AUDIT),
                    "--dynamic-result",
                    "passed",
                    "--output",
                    str(output),
                ],
                cwd=ROOT,
                check=False,
                text=True,
                capture_output=True,
            )

            self.assertIn(result.returncode, (0, 2), result.stderr)
            evidence = json.loads(output.read_text(encoding="utf-8"))

        self.assertEqual(evidence["schema_version"], 2)
        self.assertEqual(evidence["evidence_kind"], "automated")
        self.assertIs(evidence["human_review_attested"], False)
        self.assertIs(evidence["independent_audit"], False)
        self.assertNotIn("reviewer", evidence)
        self.assertNotIn("release_decision", evidence)
        self.assertIs(evidence["static_review"]["automated_inventory_only"], True)
        self.assertIs(
            evidence["static_review"]["human_data_flow_review_required"], True
        )
        self.assertNotIn("human_data_flow_reviewed", evidence["static_review"])


if __name__ == "__main__":
    unittest.main()
