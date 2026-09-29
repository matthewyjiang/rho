"""Tests for the compaction eval report summary."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from compaction_eval import summarize  # noqa: E402


def point(before, after, probes, error=None):
    return {
        "before_tokens": before,
        "after_tokens": after,
        "error": error,
        "compaction": {"output_tokens": 100, "latency_ms": 2000},
        "probes": [{"probe": name, "score": score} for name, score in probes],
    }


class SummarizeTests(unittest.TestCase):
    # Covers: failed points and unscored probes are counted, not averaged in as zeros.
    def test_failures_and_unscored_probes_stay_out_of_means(self) -> None:
        report = {
            "sessions": [
                {
                    "points": [
                        point(1000, 500, [("files_changed", 1.0), ("errors", None)]),
                        point(1000, 250, [("files_changed", 0.5), ("errors", 0.0)]),
                        point(1000, None, [], error="provider failed"),
                    ]
                }
            ]
        }

        summary = summarize(report)

        self.assertEqual(
            {
                key: summary[key]
                for key in ("points", "failures", "unscored", "score", "after_ratio")
            },
            {
                "points": 3,
                "failures": 1,
                "unscored": 1,
                "score": 0.5,
                "after_ratio": 0.375,
            },
        )
        self.assertEqual(summary["probes"]["files_changed"], 0.75)
        self.assertEqual(summary["probe_counts"]["errors"], 1)


if __name__ == "__main__":
    unittest.main()
