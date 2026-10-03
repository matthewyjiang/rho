#!/usr/bin/env python3
"""Compare permission classifier configurations on labeled and replayed cases.

Runs the hidden `rho __classifier_eval` command once per variant, saves each
JSON report, and prints a Markdown table of false allows, false denies,
errors, screen escalations, and median latency. With --baseline, also lists every case whose
verdict differs from the baseline report.

Labeled cases come from crates/rho/src/app/classifier_eval/cases.jsonl.
Replayed cases come from saved sessions, which contain user code and secrets.
They are sent only to the classifier models the variants configure, and
reports under --out hold commands and paths from them. Never commit reports.

Examples:

  # Current classifier against another model on fixtures and 20 sessions.
  scripts/classifier_eval.py --recent 20 \\
      --variant current= \\
      --variant other='--model PROVIDER/MODEL' --baseline current

  # Old build against a new build: run each with its own --rho into one --out,
  # on fixed sessions, then compare.
  scripts/classifier_eval.py --rho /tmp/rho-old --variant old= --session S1 --session S2
  scripts/classifier_eval.py --variant new= --session S1 --session S2
  scripts/classifier_eval.py --render-only --baseline old
"""

from __future__ import annotations

import argparse
import json
import re
import shlex
import statistics
import subprocess
import sys
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "crates" / "rho" / "src" / "app" / "classifier_eval" / "cases.jsonl"


def parse_variant(value: str) -> tuple[str, list[str]]:
    name, sep, args = value.partition("=")
    if not sep or not name:
        raise argparse.ArgumentTypeError("expected NAME=ARGS, for example current=")
    # The name becomes a file name under --out.
    if not re.fullmatch(r"[A-Za-z0-9_-]+", name):
        raise argparse.ArgumentTypeError(
            f"variant name {name!r} may use only letters, digits, '-', and '_'"
        )
    return name, shlex.split(args)


def recent_sessions(root: Path, count: int) -> list[Path]:
    transcripts = sorted(
        root.glob("*/*/session.jsonl"), key=lambda path: path.stat().st_mtime
    )
    return [path.parent for path in transcripts[-count:]]


def summarize(report: dict[str, Any]) -> dict[str, Any]:
    """Counts outcomes. A failed classification is an error, not a deny, even
    though production fails closed, so a broken variant cannot look safe."""
    cases = report["cases"]
    labeled = [case for case in cases if case["label"] is not None]
    decided = [case for case in cases if case["verdict"] is not None]
    false_allows = [
        case["id"]
        for case in labeled
        if case["label"] == "deny" and case["verdict"] == "allow"
    ]
    false_denies = [
        case["id"]
        for case in labeled
        if case["label"] == "allow" and case["verdict"] == "deny"
    ]
    screened = [case for case in cases if case["screen"] != "skipped"]
    return {
        "cases": len(cases),
        "labeled": len(labeled),
        "false_allows": false_allows,
        "false_denies": false_denies,
        "errors": [case["id"] for case in cases if case["error"] is not None],
        "allow_rate": (
            sum(case["verdict"] == "allow" for case in decided) / len(decided)
            if decided
            else None
        ),
        "escalation_rate": (
            sum(case["screen"] != "allow" for case in screened) / len(screened)
            if screened
            else None
        ),
        "median_latency_s": (
            statistics.median(case["latency_ms"] for case in cases) / 1000
            if cases
            else None
        ),
    }


def compare(baseline: dict[str, Any], report: dict[str, Any]) -> dict[str, Any]:
    """Verdict flips between two reports, plus every way their inputs differ.

    Flips mean something only when both reports classified the same inputs.
    `mismatched` lists cases only one report has and cases whose input digest
    changed, such as a replayed session that grew between runs. An error counts
    as a verdict of None."""
    before = {case["id"]: case for case in baseline["cases"]}
    after = {case["id"]: case for case in report["cases"]}
    mismatched = sorted(before.keys() ^ after.keys())
    flipped = []
    for case_id, case in after.items():
        old = before.get(case_id)
        if old is None:
            continue
        if old["input_digest"] != case["input_digest"]:
            mismatched.append(case_id)
        elif old["verdict"] != case["verdict"]:
            flipped.append(
                {
                    "id": case_id,
                    "label": case["label"],
                    "before": old["verdict"],
                    "after": case["verdict"],
                    "pending": case["pending"],
                }
            )
    return {"flipped": flipped, "mismatched": mismatched}


def fmt_rate(value: float | None) -> str:
    return "-" if value is None else f"{value:.0%}"


def render(summaries: dict[str, dict[str, Any]]) -> str:
    header = ["variant", "cases", "labeled", "false allow", "false deny"]
    header += ["errors", "allow rate", "escalated", "p50 latency s"]
    rows = ["| " + " | ".join(header) + " |", "|" + "---|" * len(header)]
    for name, summary in summaries.items():
        cells = [
            name,
            str(summary["cases"]),
            str(summary["labeled"]),
            str(len(summary["false_allows"])),
            str(len(summary["false_denies"])),
            str(len(summary["errors"])),
            fmt_rate(summary["allow_rate"]),
            fmt_rate(summary["escalation_rate"]),
            "-"
            if summary["median_latency_s"] is None
            else f"{summary['median_latency_s']:.1f}",
        ]
        rows.append("| " + " | ".join(cells) + " |")
    return "\n".join(rows)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument(
        "--cases",
        action="append",
        type=Path,
        help=f"labeled case file; default {FIXTURES.relative_to(ROOT)}",
    )
    parser.add_argument("--no-cases", action="store_true", help="replay sessions only")
    parser.add_argument("--session", action="append", default=[], type=Path)
    parser.add_argument(
        "--recent",
        type=int,
        metavar="N",
        help="also replay the N most recently updated sessions under --sessions-root",
    )
    parser.add_argument(
        "--sessions-root", type=Path, default=Path.home() / ".rho" / "sessions"
    )
    parser.add_argument("--per-session", type=int, default=5)
    parser.add_argument(
        "--variant",
        action="append",
        type=parse_variant,
        default=[],
        metavar="NAME=ARGS",
        help="__classifier_eval arguments for one configuration; repeat to compare",
    )
    parser.add_argument(
        "--rho-args", default="", help="root rho arguments for every variant"
    )
    parser.add_argument(
        "--jobs",
        type=int,
        default=1,
        help="cases classified at once per variant; more inflates latency",
    )
    parser.add_argument("--rho", type=Path, default=ROOT / "target" / "debug" / "rho")
    parser.add_argument("--out", type=Path, default=Path("/tmp/rho-classifier-eval"))
    parser.add_argument(
        "--baseline", metavar="NAME", help="list verdict flips against this report"
    )
    parser.add_argument(
        "--render-only",
        action="store_true",
        help="skip runs and render the reports already under --out",
    )
    args = parser.parse_args()

    args.out.mkdir(parents=True, exist_ok=True)
    args.out.chmod(0o700)
    reports = sorted(args.out.glob("*.json"))
    # A run renders only its own variants unless --baseline asks for a
    # comparison; --render-only redraws every saved report.
    if not args.render_only:
        if not args.variant:
            parser.error("need at least one --variant")
        case_files = [] if args.no_cases else (args.cases or [FIXTURES])
        sessions = list(args.session)
        if args.recent:
            sessions += recent_sessions(args.sessions_root, args.recent)
        running = []
        for name, variant_args in args.variant:
            command = [str(args.rho), *shlex.split(args.rho_args), "__classifier_eval"]
            command += ["--jobs", str(args.jobs), "--per-session", str(args.per_session)]
            command += variant_args
            for path in case_files:
                command += ["--cases", str(path)]
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
        # A comparison needs the baseline's saved report too.
        reports = (
            sorted(args.out.glob("*.json"))
            if args.baseline is not None
            else [args.out / f"{name}.json" for name, _ in args.variant]
        )

    loaded = {path.stem: json.loads(path.read_text()) for path in reports}
    summaries = {}
    for name, report in loaded.items():
        summaries[name] = summarize(report)
        print(
            f"{name}: {report['model']} auth={report['auth']} "
            f"reasoning={report['reasoning']}",
            file=sys.stderr,
        )
    print(render(summaries))
    for name, summary in summaries.items():
        for kind in ("false_allows", "false_denies", "errors"):
            if summary[kind]:
                print(f"\n{name} {kind.replace('_', ' ')}: {', '.join(summary[kind])}")
    if args.baseline is not None:
        if args.baseline not in loaded:
            print(f"no report named {args.baseline} under {args.out}", file=sys.stderr)
            return 1
        mismatched_inputs = False
        for name, report in loaded.items():
            if name == args.baseline:
                continue
            result = compare(loaded[args.baseline], report)
            print(f"\n{name} vs {args.baseline}: {len(result['flipped'])} flipped")
            for flip in result["flipped"]:
                print(
                    f"- {flip['id']} [{flip['label'] or 'unlabeled'}] "
                    f"{flip['before']} -> {flip['after']}: {flip['pending']}"
                )
            if result["mismatched"]:
                mismatched_inputs = True
                print(
                    f"{len(result['mismatched'])} cases differ in input; "
                    f"flips exclude them: {', '.join(result['mismatched'])}"
                )
        if mismatched_inputs:
            return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
