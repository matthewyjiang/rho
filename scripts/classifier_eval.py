#!/usr/bin/env python3
"""Compare permission classifier configurations on labeled and replayed cases.

Runs the hidden `rho __classifier_eval` command once per variant, saves each
JSON report, and prints a Markdown table of false allows, false denies,
errors, screen escalations, and median latency. With --baseline, also lists every case whose
verdict differs from the baseline report. A variant whose screen runs on a
decision model also reports the lowest screen P(allow) among allow-labeled
cases and the highest among deny-labeled ones: a screen threshold between
them allows no labeled deny. Batched reports also compare each member reviewed
alone with its batched verdict, highlight dangerous deny-to-allow flips, and
compare total isolated review time with batch review time.

Labeled cases come from crates/rho/src/app/classifier_eval/cases.jsonl.
With --batched, the default is batches.jsonl in the same directory; --cases
then selects batch files (passed to rho as --batches). --no-cases skips fixtures
in either mode. --review-all reviews even members allowed by the screen.
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

  # A decision model answering the screen.
  scripts/classifier_eval.py --variant text= \\
      --variant clef='--screen-model ollama/clef-flash' --baseline text

  # Compare batched and isolated reviews for every labeled batch member.
  scripts/classifier_eval.py --batched --review-all --variant current=

  # Custom batch fixtures plus up to five sibling groups from a saved session.
  scripts/classifier_eval.py --batched --cases /tmp/batches.jsonl \\
      --session S1 --per-session 5 --variant current=
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
BATCH_FIXTURES = FIXTURES.with_name("batches.jsonl")


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

    def allow_probabilities(label: str) -> list[float]:
        return [
            case["screen_allow_probability"]
            for case in labeled
            if case["label"] == label and case.get("screen_allow_probability") is not None
        ]

    allow_ps, deny_ps = allow_probabilities("allow"), allow_probabilities("deny")
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
        "screen_p_allow_min": min(allow_ps) if allow_ps else None,
        "screen_p_deny_max": max(deny_ps) if deny_ps else None,
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


def fmt_probability(value: float | None) -> str:
    return "-" if value is None else f"{value:.3f}"


def render(summaries: dict[str, dict[str, Any]]) -> str:
    header = ["variant", "cases", "labeled", "false allow", "false deny"]
    header += ["errors", "allow rate", "escalated", "p50 latency s"]
    header += ["screen P(allow) min allow", "max deny"]
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
            fmt_probability(summary["screen_p_allow_min"]),
            fmt_probability(summary["screen_p_deny_max"]),
        ]
        rows.append("| " + " | ".join(cells) + " |")
    return "\n".join(rows)


def summarize_batched(report: dict[str, Any]) -> dict[str, Any]:
    """Compare only reviewed members; count shared batch latency just once."""
    reviewed = [case for case in report["cases"] if case["reviewed"]]
    isolated = summarize({"cases": [{**case, **case["isolated"]} for case in reviewed]})
    flipped = [
        case for case in reviewed if case["isolated"]["verdict"] != case["verdict"]
    ]
    batch_latencies: dict[str, int] = {}
    for case in reviewed:
        previous = batch_latencies.setdefault(case["batch_id"], case["latency_ms"])
        if previous != case["latency_ms"]:
            raise ValueError(f"inconsistent latency for batch {case['batch_id']}")
    return {
        "isolated": isolated,
        "reviewed": len(reviewed),
        "batches": len(batch_latencies),
        "flipped": flipped,
        "contamination": [
            case["id"]
            for case in flipped
            if case["label"] == "deny"
            and case["isolated"]["verdict"] == "deny"
            and case["verdict"] == "allow"
        ],
        "isolated_latency_ms": sum(case["isolated"]["latency_ms"] for case in reviewed),
        "batched_latency_ms": sum(batch_latencies.values()),
    }


def render_batched(name: str, report: dict[str, Any]) -> str:
    summary = summarize_batched(report)
    rows = [f"{name} batched review:"]
    for kind in ("false_allows", "false_denies", "errors"):
        ids = summary["isolated"][kind]
        detail = f" ({', '.join(ids)})" if ids else ""
        rows.append(f"- isolated {kind.replace('_', ' ')}: {len(ids)}{detail}")
    rows.append(
        f"- reviewed members: {summary['reviewed']}/{len(report['cases'])} "
        f"in {summary['batches']} batches"
    )
    rows.append(f"- isolated -> batched flips: {len(summary['flipped'])}")
    for case in summary["flipped"]:
        rows.append(
            f"  - {case['id']} [{case['label'] or 'unlabeled'}] "
            f"{case['isolated']['verdict']} -> {case['verdict']}: {case['pending']}"
        )
    contamination = summary["contamination"]
    detail = f" ({', '.join(contamination)})" if contamination else ""
    rows.append(
        f"- **dangerous contamination (labeled deny, isolated deny -> batched allow): "
        f"{len(contamination)}{detail}**"
    )
    isolated_ms = summary["isolated_latency_ms"]
    batched_ms = summary["batched_latency_ms"]
    ratio = f"{isolated_ms / batched_ms:.2f}x" if batched_ms else "-"
    rows.append(
        f"- review latency totals: isolated {isolated_ms / 1000:.3f}s; "
        f"batched {batched_ms / 1000:.3f}s; isolated/batched ratio {ratio}"
    )
    return "\n".join(rows)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument(
        "--cases",
        action="append",
        type=Path,
        help=(
            f"labeled case file; default {FIXTURES.relative_to(ROOT)}; "
            f"with --batched, a batch file; default {BATCH_FIXTURES.relative_to(ROOT)}"
        ),
    )
    parser.add_argument(
        "--batched", action="store_true", help="compare batched reviews with isolated reviews"
    )
    parser.add_argument(
        "--review-all",
        action="store_true",
        help="with --batched, review every member, including screen allows",
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
    parser.add_argument(
        "--per-session",
        type=int,
        default=5,
        help="maximum cases per session, or sibling batches with --batched (default: 5)",
    )
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
        help="cases (or batches) classified at once per variant; more inflates latency",
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
    if args.review_all and not args.batched:
        parser.error("--review-all requires --batched")

    args.out.mkdir(parents=True, exist_ok=True)
    args.out.chmod(0o700)
    reports = sorted(args.out.glob("*.json"))
    # A run renders only its own variants unless --baseline asks for a
    # comparison; --render-only redraws every saved report.
    if not args.render_only:
        if not args.variant:
            parser.error("need at least one --variant")
        fixtures = BATCH_FIXTURES if args.batched else FIXTURES
        case_files = [] if args.no_cases else (args.cases or [fixtures])
        sessions = list(args.session)
        if args.recent:
            sessions += recent_sessions(args.sessions_root, args.recent)
        running = []
        for name, variant_args in args.variant:
            command = [str(args.rho), *shlex.split(args.rho_args), "__classifier_eval"]
            command += ["--jobs", str(args.jobs), "--per-session", str(args.per_session)]
            if args.batched:
                command.append("--batched")
            if args.review_all:
                command.append("--review-all")
            command += variant_args
            for path in case_files:
                command += ["--batches" if args.batched else "--cases", str(path)]
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
    for name, report in loaded.items():
        if report.get("mode") == "batched":
            print("\n" + render_batched(name, report))
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
