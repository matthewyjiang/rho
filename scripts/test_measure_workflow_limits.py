#!/usr/bin/env python3
"""Fast CI checks for the workflow limit corpus and receipt schema."""

from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from measure_workflow_limits import (
    DETERMINISTIC_FIELDS,
    UNMEASURED_FIELDS,
    main,
    verify_arithmetic,
    verify_rust_constants,
)
from workflow_limit_corpus import generate_corpus, source_request

ROOT = Path(__file__).resolve().parents[1]
RECEIPT = ROOT / "crates/rho/src/workflow/fixtures/limit_receipt.json"
MANIFEST = ROOT / "crates/rho/src/workflow/fixtures/limit_corpus.json"


class WorkflowLimitReceiptTests(unittest.TestCase):
    # Covers: receipt drift must not discard the measurements needed to update it.
    # Owner: workflow limit measurement tooling; corpus checks do not exercise CLI output.
    def test_json_output_survives_receipt_mismatch(self) -> None:
        receipt = json.loads(RECEIPT.read_text())
        measured = dict(receipt["planning"]["measured"])
        measured["graph_bytes"] += 1
        process = {
            name: values["measured"]
            for name, values in receipt["planner_process"].items()
        }
        cases = {"graph_bytes": measured}
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "measurements.json"
            with patch(
                "sys.argv", ["measure_workflow_limits.py", "--rho", "rho", "--json-output", str(output)]
            ), patch(
                "measure_workflow_limits.measure_corpus", return_value=(measured, process, cases)
            ), self.assertRaises(SystemExit):
                main()
            self.assertEqual(
                json.loads(output.read_text()),
                {"planning": measured, "planner_process": process, "cases": cases},
            )

    # Covers: receipt values could drift from the deterministic corpus shape.
    # Owner: workflow limit measurement tooling.
    def test_generator_is_deterministic_and_covers_each_budget(self) -> None:
        with tempfile.TemporaryDirectory() as left_dir, tempfile.TemporaryDirectory() as right_dir:
            left_root = Path(left_dir)
            right_root = Path(right_dir)
            left = generate_corpus(left_root)
            right = generate_corpus(right_root)
            self.assertEqual([case.name for case in left], [case.name for case in right])
            self.assertEqual(
                [source_request(case, left_root) for case in left],
                [source_request(case, right_root) for case in right],
            )

        manifest = json.loads(MANIFEST.read_text())
        covered = {
            name
            for names in manifest["cases"].values()
            for name in names
            if name in DETERMINISTIC_FIELDS
        }
        self.assertEqual(covered, DETERMINISTIC_FIELDS)

    # Covers: a hand-edited receipt could have a silent gap or false margin.
    # Owner: workflow limit measurement tooling.
    def test_receipt_arithmetic_and_fields(self) -> None:
        receipt = json.loads(RECEIPT.read_text())
        verify_arithmetic(receipt)
        verify_rust_constants(receipt)
        self.assertEqual(
            set(receipt["planning"]["measured"]) - {"worker_wall_millis"},
            DETERMINISTIC_FIELDS | UNMEASURED_FIELDS,
        )


if __name__ == "__main__":
    unittest.main()
