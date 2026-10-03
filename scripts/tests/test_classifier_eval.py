"""Tests for the classifier eval report summary and comparison."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from classifier_eval import compare, summarize  # noqa: E402


def case(
    case_id, label, verdict, screen="escalate", error=None, latency_ms=1000, digest="d"
):
    return {
        "latency_ms": latency_ms,
        "input_digest": digest,
        "id": case_id,
        "label": label,
        "verdict": verdict,
        "screen": screen,
        "error": error,
        "pending": f"cmd {case_id}",
    }


class SummarizeTests(unittest.TestCase):
    # Covers: errors are reported as errors, not counted as safe denies, and
    # unlabeled replay cases never count as false allows or denies.
    def test_errors_and_unlabeled_cases_stay_out_of_label_metrics(self) -> None:
        report = {
            "cases": [
                case("a", "deny", "allow", screen="allow"),
                case("b", "allow", "deny"),
                case("c", "deny", None, screen="skipped", error="over budget"),
                case("d", None, "allow", screen="allow", latency_ms=3000),
            ]
        }

        summary = summarize(report)

        self.assertEqual(
            summary,
            {
                "cases": 4,
                "labeled": 3,
                "false_allows": ["a"],
                "false_denies": ["b"],
                "errors": ["c"],
                "allow_rate": 2 / 3,
                "escalation_rate": 1 / 3,
                "median_latency_s": 1.0,
            },
        )


class CompareTests(unittest.TestCase):
    # Covers: a comparison lists changed verdicts, including a new error, and
    # reports cases it cannot compare instead of silently dropping them.
    def test_flips_include_errors_and_mismatched_inputs_are_reported(self) -> None:
        baseline = {
            "cases": [
                case("a", None, "allow"),
                case("b", "deny", "deny"),
                case("grew", None, "allow"),
                case("gone", None, "allow"),
            ]
        }
        report = {
            "cases": [
                case("a", None, None, error="provider down"),
                case("b", "deny", "deny"),
                case("grew", None, "deny", digest="other"),
                case("new", None, "allow"),
            ]
        }

        self.assertEqual(
            compare(baseline, report),
            {
                "flipped": [
                    {
                        "id": "a",
                        "label": None,
                        "before": "allow",
                        "after": None,
                        "pending": "cmd a",
                    }
                ],
                "mismatched": ["gone", "new", "grew"],
            },
        )


if __name__ == "__main__":
    unittest.main()
