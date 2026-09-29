#!/usr/bin/env python3
"""Compare compaction configurations on saved Rho sessions.

Runs the hidden `rho __compaction_eval` command once per variant, saves each
JSON report, and prints a Markdown table comparing probe scores, context
reduction, compaction tokens, cost, and latency.

Sessions contain user code and secrets. They are sent only to the models the
variants configure, and reports are written only under --out, which holds
transcript excerpts and should stay local. Never commit reports.

Examples:

  # Default settings against a smaller target, on 5 recent sessions.
  scripts/compaction_eval.py --recent 5 \\
      --variant 'none=--tiers none' \\
      --variant default= \\
      --variant target40='--target-percent 40'

  # Re-render saved reports.
  scripts/compaction_eval.py --out /tmp/ce --render-only
"""

from __future__ import annotations

import argparse
import json
import shlex
import statistics
import subprocess
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
PROBES = ("files_changed", "test_result", "user_requests", "errors")


def parse_variant(value: str) -> tuple[str, list[str]]:
    name, sep, args = value.partition("=")
    if not sep or not name:
        raise argparse.ArgumentTypeError("expected NAME=ARGS, for example default=")
    return name, shlex.split(args)


def recent_sessions(root: Path, count: int) -> list[Path]:
    transcripts = sorted(
        root.glob("*/*/session.jsonl"), key=lambda path: path.stat().st_mtime
    )
    return [path.parent for path in transcripts[-count:]]


def summarize(report: dict[str, Any]) -> dict[str, Any]:
    """Aggregates one report. Probe means skip unscored probes."""
    scores: dict[str, list[float]] = {probe: [] for probe in PROBES}
    ratios, output_tokens, costs, latencies = [], [], [], []
    points = failures = unscored = 0
    for session in report["sessions"]:
        for point in session["points"]:
            points += 1
            record = point.get("compaction") or {}
            if point.get("error") or point.get("after_tokens") is None:
                failures += 1
                continue
            ratios.append(point["after_tokens"] / point["before_tokens"])
            output_tokens.append(record.get("output_tokens") or 0)
            if record.get("cost_usd_micros") is not None:
                costs.append(record["cost_usd_micros"] / 1_000_000)
            latencies.append(record.get("latency_ms", 0) / 1000)
            for probe in point["probes"]:
                if probe["score"] is None:
                    unscored += 1
                else:
                    scores[probe["probe"]].append(probe["score"])
    all_scores = [score for values in scores.values() for score in values]
    mean = lambda values: statistics.fmean(values) if values else None
    return {
        "points": points,
        "failures": failures,
        "unscored": unscored,
        "score": mean(all_scores),
        "probes": {probe: mean(values) for probe, values in scores.items()},
        "probe_counts": {probe: len(values) for probe, values in scores.items()},
        "after_ratio": mean(ratios),
        "output_tokens": mean(output_tokens),
        "cost_usd": sum(costs) if costs else None,
        "latency_s": mean(latencies),
    }


def fmt(value: Any, digits: int = 2) -> str:
    if value is None:
        return "-"
    if isinstance(value, float):
        return f"{value:.{digits}f}"
    return str(value)


def render(summaries: dict[str, dict[str, Any]]) -> str:
    header = ["variant", "points", "fail", "score", *PROBES]
    header += ["after/before", "summary out tok", "cost $", "latency s"]
    rows = ["| " + " | ".join(header) + " |", "|" + "---|" * len(header)]
    for name, summary in summaries.items():
        probes = [
            f"{fmt(summary['probes'][probe])} (n={summary['probe_counts'][probe]})"
            for probe in PROBES
        ]
        cells = [
            name,
            fmt(summary["points"]),
            fmt(summary["failures"]),
            fmt(summary["score"]),
            *probes,
            fmt(summary["after_ratio"]),
            fmt(summary["output_tokens"], 0),
            fmt(summary["cost_usd"], 4),
            fmt(summary["latency_s"], 1),
        ]
        rows.append("| " + " | ".join(cells) + " |")
    return "\n".join(rows)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--session", action="append", default=[], type=Path)
    parser.add_argument(
        "--recent",
        type=int,
        metavar="N",
        help="also use the N most recently updated sessions under --sessions-root",
    )
    parser.add_argument(
        "--sessions-root", type=Path, default=Path.home() / ".rho" / "sessions"
    )
    parser.add_argument(
        "--variant",
        action="append",
        type=parse_variant,
        default=[],
        metavar="NAME=ARGS",
        help="__compaction_eval arguments for one configuration; repeat to compare",
    )
    parser.add_argument(
        "--rho-args",
        default="",
        help="root rho arguments for every variant, e.g. '--provider X --model Y'",
    )
    parser.add_argument("--points", type=int, default=3)
    parser.add_argument(
        "--jobs", type=int, default=8, help="points each variant replays at once"
    )
    parser.add_argument("--rho", type=Path, default=ROOT / "target" / "debug" / "rho")
    parser.add_argument("--out", type=Path, default=Path("/tmp/rho-compaction-eval"))
    parser.add_argument(
        "--render-only",
        action="store_true",
        help="skip runs and render the reports already under --out",
    )
    args = parser.parse_args()

    args.out.mkdir(parents=True, exist_ok=True)
    args.out.chmod(0o700)
    # A run renders only its own variants; --render-only redraws every saved report.
    reports = sorted(args.out.glob("*.json"))
    if not args.render_only:
        sessions = list(args.session)
        if args.recent:
            sessions += recent_sessions(args.sessions_root, args.recent)
        if not sessions or not args.variant:
            parser.error("need at least one --session or --recent, and one --variant")
        # Variants are independent processes; run them all at once. Each
        # also replays --jobs points at a time.
        running = []
        for name, variant_args in args.variant:
            command = [str(args.rho), *shlex.split(args.rho_args), "__compaction_eval"]
            command += ["--points", str(args.points), "--jobs", str(args.jobs)]
            command += variant_args
            for session in sessions:
                command += ["--session", str(session)]
            report_path = args.out / f"{name}.json"
            log_path = args.out / f"{name}.log"
            print(f"running {name}", file=sys.stderr)
            with report_path.open("wb") as report, log_path.open("wb") as log:
                process = subprocess.Popen(command, stdout=report, stderr=log)
            report_path.chmod(0o600)
            log_path.chmod(0o600)
            running.append((name, process, log_path))
        failed = [name for name, process, _ in running if process.wait() != 0]
        for name, _, log_path in running:
            if name in failed:
                print(f"{name} failed; see {log_path}", file=sys.stderr)
        if failed:
            return 1
        reports = [args.out / f"{name}.json" for name, _ in args.variant]

    summaries = {}
    for path in reports:
        report = json.loads(path.read_text())
        summaries[path.stem] = summarize(report)
        print(f"{path.stem}: {json.dumps(report['configuration'])}", file=sys.stderr)
    print(render(summaries))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
