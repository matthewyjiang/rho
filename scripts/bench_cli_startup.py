#!/usr/bin/env python3
"""Benchmark CLI process overhead and render a comparison chart.

Measures wall time and peak RSS for each tool's help invocation.
This is CLI process overhead only, not interactive TUI cost or model latency.

Linux only (uses os.wait4 ru_maxrss).
"""

from __future__ import annotations

import argparse
import html
import json
import os
import platform
import re
import shutil
import statistics
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any


def repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def default_rho_bin() -> Path | None:
    release = repo_root() / "target" / "release" / "rho"
    if release.exists():
        return release
    path = shutil.which("rho")
    return Path(path) if path else None


def rho_version(_rho_bin: Path) -> str:
    cargo_toml = repo_root() / "crates" / "rho" / "Cargo.toml"
    if cargo_toml.exists():
        match = re.search(r'^version\s*=\s*"([^"]+)"', cargo_toml.read_text(), re.M)
        if match:
            return f"rho {match.group(1)}"
    return "rho"


def capture_version(cmd: list[str]) -> str:
    try:
        proc = subprocess.run(
            cmd,
            check=False,
            text=True,
            capture_output=True,
            timeout=30,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        return str(exc)
    lines = (proc.stdout or proc.stderr or "").strip().splitlines()
    return lines[0] if lines else f"exit {proc.returncode}"


def percentile(sorted_values: list[float], p: float) -> float:
    if not sorted_values:
        raise ValueError("no samples")
    if len(sorted_values) == 1:
        return sorted_values[0]
    rank = (len(sorted_values) - 1) * p
    low = int(rank)
    high = min(low + 1, len(sorted_values) - 1)
    if low == high:
        return sorted_values[low]
    frac = rank - low
    return sorted_values[low] + (sorted_values[high] - sorted_values[low]) * frac


def summarize(values: list[float]) -> dict[str, float]:
    ordered = sorted(values)
    return {
        "min": ordered[0],
        "median": statistics.median(ordered),
        "mean": statistics.fmean(ordered),
        "p95": percentile(ordered, 0.95),
        "max": ordered[-1],
    }


def run_once(cmd: list[str]) -> tuple[float, int, int]:
    start = time.perf_counter()
    pid = os.fork()
    if pid == 0:
        try:
            devnull = os.open(os.devnull, os.O_RDWR)
            os.dup2(devnull, 0)
            os.dup2(devnull, 1)
            os.dup2(devnull, 2)
            os.execvp(cmd[0], cmd)
        except OSError:
            os._exit(127)
    _pid, status, rusage = os.wait4(pid, 0)
    elapsed_ms = (time.perf_counter() - start) * 1000.0
    return elapsed_ms, int(rusage.ru_maxrss), os.waitstatus_to_exitcode(status)


def measure(cmd: list[str], *, warmup: int, samples: int) -> dict[str, Any]:
    for _ in range(warmup):
        run_once(cmd)
    times_ms: list[float] = []
    rss_kib: list[float] = []
    for _ in range(samples):
        elapsed_ms, max_rss_kib, _rc = run_once(cmd)
        times_ms.append(elapsed_ms)
        rss_kib.append(float(max_rss_kib))
    return {
        "samples": samples,
        "time_ms": summarize(times_ms),
        "rss_kib": summarize(rss_kib),
    }


@dataclass(frozen=True)
class Candidate:
    name: str
    label: str
    args: list[str]
    version_args: list[str] | None = None
    highlight: bool = False


def build_candidates(rho_bin: Path) -> list[Candidate]:
    return [
        Candidate("rho", "rho", [str(rho_bin), "--help"], highlight=True),
        Candidate("codex", "Codex", ["codex", "--help"]),
        Candidate("claude", "Claude Code", ["claude", "--help"]),
        Candidate(
            "pi",
            "Pi (no extensions)",
            ["pi", "--no-extensions", "--help"],
            version_args=["pi", "--version"],
        ),
        Candidate("opencode", "OpenCode", ["opencode", "--help"]),
    ]


def resolve_binary(arg0: str) -> str | None:
    if os.path.isabs(arg0):
        return arg0 if Path(arg0).exists() else None
    return shutil.which(arg0)


def resolve_candidate(candidate: Candidate) -> dict[str, Any] | None:
    binary = resolve_binary(candidate.args[0])
    if binary is None:
        return None
    cmd = [binary, *candidate.args[1:]]
    version_cmd = (
        [resolve_binary(candidate.version_args[0]) or candidate.version_args[0], *candidate.version_args[1:]]
        if candidate.version_args
        else [binary, "--version"]
    )
    if candidate.name == "rho":
        version = rho_version(Path(binary))
    else:
        version = capture_version(version_cmd)
    real = os.path.realpath(binary)
    return {
        "name": candidate.name,
        "label": candidate.label,
        "highlight": candidate.highlight,
        "cmd": cmd,
        "bin": binary,
        "real_bin": real,
        "size_bytes": Path(real).stat().st_size,
        "version": version,
    }


def fmt_ms(value: float) -> str:
    if value >= 100:
        return f"{value:.0f} ms"
    if value >= 10:
        return f"{value:.1f} ms"
    return f"{value:.2f} ms"


def fmt_mib(kib: float) -> str:
    return f"{kib / 1024:.0f} MiB" if kib >= 100 * 1024 else f"{kib / 1024:.1f} MiB"


def print_table(results: list[dict[str, Any]]) -> None:
    headers = ("tool", "median", "p95", "rss median", "rss p95", "version")
    rows: list[tuple[str, ...]] = []
    for item in results:
        rows.append(
            (
                item["label"],
                fmt_ms(item["time_ms"]["median"]),
                fmt_ms(item["time_ms"]["p95"]),
                fmt_mib(item["rss_kib"]["median"]),
                fmt_mib(item["rss_kib"]["p95"]),
                item["version"],
            )
        )
    widths = [len(h) for h in headers]
    for row in rows:
        for idx, cell in enumerate(row):
            widths[idx] = max(widths[idx], len(cell))

    def fmt_row(cols: tuple[str, ...]) -> str:
        return "  ".join(cell.ljust(widths[idx]) for idx, cell in enumerate(cols))

    print(fmt_row(headers))
    print(fmt_row(tuple("-" * w for w in widths)))
    for row in rows:
        print(fmt_row(row))


def render_svg(results: list[dict[str, Any]], *, samples: int) -> str:
    # Compact side-by-side panels with fixed value columns so labels never collide.
    width = 1000
    pad_x = 20
    label_w = 148
    value_w = 68
    gutter = 24
    header_h = 54
    footer_h = 26
    row_h = 28
    bar_h = 11
    panel_title_h = 16

    n = len(results)
    chart_h = n * row_h
    height = header_h + panel_title_h + chart_h + footer_h

    usable = width - (pad_x * 2) - label_w - gutter
    panel_w = usable / 2
    plot_w = panel_w - value_w - 12

    left_plot_x = pad_x + label_w
    left_value_x = left_plot_x + plot_w + 8
    right_plot_x = left_value_x + value_w + gutter
    right_value_x = right_plot_x + plot_w + 8

    rows_top = header_h + panel_title_h
    max_ms = max(item["time_ms"]["median"] for item in results)
    max_mib = max(item["rss_kib"]["median"] / 1024 for item in results)
    ms_scale_max = max(max_ms * 1.05, 1.0)
    mib_scale_max = max(max_mib * 1.05, 1.0)

    def bar_width(value: float, scale_max: float) -> float:
        return max(3.0, (value / scale_max) * plot_w)

    def row_center_y(index: int) -> float:
        return rows_top + index * row_h + (row_h / 2)

    parts: list[str] = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" role="img" aria-labelledby="title desc">',
        '  <title id="title">CLI startup and memory comparison</title>',
        '  <desc id="desc">Side-by-side bar chart of median help startup time and peak RSS for rho and other agent CLIs on Linux.</desc>',
        "  <defs>",
        "    <style>",
        '      .sans { font-family: "DejaVu Sans", "Segoe UI", Helvetica, Arial, sans-serif; }',
        '      .mono { font-family: "DejaVu Sans Mono", "Liberation Mono", Consolas, monospace; }',
        "      .title { fill: #f0f3f6; font-size: 18px; font-weight: 700; }",
        "      .subtitle { fill: #8b949e; font-size: 12px; }",
        "      .panel { fill: #8b949e; font-size: 11px; letter-spacing: 0.05em; }",
        "      .label { fill: #c9d1d9; font-size: 12px; }",
        "      .label-hi { fill: #39c5cf; font-size: 12px; font-weight: 700; }",
        "      .value { fill: #c9d1d9; font-size: 12px; }",
        "      .value-hi { fill: #3fb950; font-size: 12px; font-weight: 700; }",
        "      .foot { fill: #6e7681; font-size: 11px; }",
        "      .bar { fill: #30363d; }",
        "      .bar-hi { fill: #39c5cf; }",
        "      .track { fill: #161b22; }",
        "    </style>",
        "  </defs>",
        '  <rect width="100%" height="100%" rx="12" fill="#0d1117"/>',
        f'  <text x="{pad_x}" y="24" class="sans title">CLI process overhead</text>',
        f'  <text x="{pad_x}" y="44" class="sans subtitle">Median of {samples} help-startup runs on Linux x86_64. Not TUI cost or model latency.</text>',
        f'  <text x="{left_plot_x}" y="{header_h + 2}" class="sans panel">STARTUP</text>',
        f'  <text x="{right_plot_x}" y="{header_h + 2}" class="sans panel">PEAK RSS</text>',
    ]

    for index, item in enumerate(results):
        cy = row_center_y(index)
        bar_y = cy - (bar_h / 2)
        text_y = cy + 4
        label_class = "label-hi" if item["highlight"] else "label"
        bar_class = "bar-hi" if item["highlight"] else "bar"
        value_class = "value-hi" if item["highlight"] else "value"
        label = html.escape(item["label"])

        ms = item["time_ms"]["median"]
        mib = item["rss_kib"]["median"] / 1024
        ms_w = bar_width(ms, ms_scale_max)
        mib_w = bar_width(mib, mib_scale_max)

        # Shared labels once on the left.
        parts.append(
            f'  <text x="{left_plot_x - 10}" y="{text_y:.1f}" text-anchor="end" class="sans {label_class}">{label}</text>'
        )

        # Startup track + bar + fixed value column.
        parts.append(
            f'  <rect x="{left_plot_x}" y="{bar_y:.1f}" width="{plot_w:.1f}" height="{bar_h}" rx="3" class="track"/>'
        )
        parts.append(
            f'  <rect x="{left_plot_x}" y="{bar_y:.1f}" width="{ms_w:.1f}" height="{bar_h}" rx="3" class="{bar_class}"/>'
        )
        parts.append(
            f'  <text x="{left_value_x + value_w}" y="{text_y:.1f}" text-anchor="end" class="mono {value_class}">{fmt_ms(ms)}</text>'
        )

        # Memory track + bar + fixed value column.
        parts.append(
            f'  <rect x="{right_plot_x}" y="{bar_y:.1f}" width="{plot_w:.1f}" height="{bar_h}" rx="3" class="track"/>'
        )
        parts.append(
            f'  <rect x="{right_plot_x}" y="{bar_y:.1f}" width="{mib_w:.1f}" height="{bar_h}" rx="3" class="{bar_class}"/>'
        )
        parts.append(
            f'  <text x="{right_value_x + value_w}" y="{text_y:.1f}" text-anchor="end" class="mono {value_class}">{fmt_mib(item["rss_kib"]["median"])}</text>'
        )

    foot_y = height - 10
    parts.append(
        f'  <text x="{pad_x}" y="{foot_y}" class="sans foot">Native: rho, Codex. JS runtimes: Claude Code, Pi, OpenCode. Pi uses --no-extensions.</text>'
    )
    parts.append("</svg>")
    return "\n".join(parts) + "\n"


def main() -> int:
    if not hasattr(os, "wait4") or not hasattr(os, "fork"):
        print("this benchmark requires Linux fork/wait4 support", file=sys.stderr)
        return 2

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--rho",
        type=Path,
        default=None,
        help="path to rho binary (default: target/release/rho or PATH)",
    )
    parser.add_argument("--warmup", type=int, default=5)
    parser.add_argument("--samples", type=int, default=50)
    parser.add_argument(
        "--json",
        type=Path,
        default=None,
        help="optional path to write machine-readable results",
    )
    parser.add_argument(
        "--svg",
        type=Path,
        default=repo_root() / "docs" / "assets" / "cli-startup.svg",
        help="path to write the comparison chart SVG",
    )
    parser.add_argument("--no-svg", action="store_true", help="skip SVG output")
    args = parser.parse_args()

    rho_bin = args.rho or default_rho_bin()
    if rho_bin is None:
        print("rho binary not found; pass --rho or build target/release/rho", file=sys.stderr)
        return 2
    rho_bin = rho_bin.resolve()
    if not rho_bin.exists():
        print(f"rho binary not found: {rho_bin}", file=sys.stderr)
        return 2

    results: list[dict[str, Any]] = []
    for candidate in build_candidates(rho_bin):
        resolved = resolve_candidate(candidate)
        if resolved is None:
            print(f"skip {candidate.name}: not found", file=sys.stderr)
            continue
        print(f"bench {resolved['label']} ...", file=sys.stderr)
        measured = measure(resolved["cmd"], warmup=args.warmup, samples=args.samples)
        results.append({**resolved, **measured})

    if not results:
        print("no tools measured", file=sys.stderr)
        return 2

    # Fastest first so the chart reads top-to-bottom as lightest overhead.
    results.sort(key=lambda item: (item["time_ms"]["median"], item["rss_kib"]["median"]))

    payload = {
        "host": {
            "os": f"{platform.system()} {platform.release()}",
            "machine": platform.machine(),
            "python": platform.python_version(),
        },
        "method": {
            "rho": "rho --help",
            "codex": "codex --help",
            "claude": "claude --help",
            "pi": "pi --no-extensions --help",
            "opencode": "opencode --help",
            "warmup": args.warmup,
            "samples": args.samples,
            "time": "wall clock around fork/exec/wait4",
            "rss": "ru_maxrss KiB from wait4 (Linux)",
            "scope": "CLI process overhead for help startup only; not interactive TUI or model latency",
        },
        "results": results,
    }

    print_table(results)
    print()
    print("scope: help startup process overhead only")
    print("pi flags: --no-extensions")
    print("not measured: interactive TUI startup, tool execution, or model latency")

    if not args.no_svg:
        svg_path = args.svg
        svg_path.parent.mkdir(parents=True, exist_ok=True)
        svg_path.write_text(render_svg(results, samples=args.samples))
        print(f"wrote {svg_path}")

    if args.json:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(json.dumps(payload, indent=2) + "\n")
        print(f"wrote {args.json}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
