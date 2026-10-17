#!/usr/bin/env python3
"""Run the common local validation workflows."""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence

ROOT = Path(__file__).resolve().parents[1]
# 12 equalled nproc on the development host, so the old cap never
# reduced rustc 1.92 query ICEs.
MAX_CARGO_JOBS = 8
# rustc 1.92.0 default compile-thread stack is 8 MiB; it asks for this
# exact floor after SIGSEGV / query ICEs on this workspace.
RUSTC_MIN_STACK_BYTES = 16 * 1024 * 1024


@dataclass(frozen=True)
class Step:
    """One named validation command."""

    label: str
    command: tuple[str, ...]


def fast_plan(
    package: str,
    *,
    lib: bool = False,
    integration_test: str | None = None,
    test_filter: str | None = None,
) -> tuple[Step, ...]:
    """Plan quick checks for one package and an optional test selection."""
    steps = [
        Step("Check formatting", ("cargo", "fmt", "--all", "--", "--check")),
        Step(
            "Check architecture guardrails",
            ("python3", "scripts/check_architecture.py"),
        ),
        Step(
            f"Check {package}",
            ("cargo", "check", "-p", package, "--locked"),
        ),
    ]

    if lib or integration_test or test_filter:
        test_command = ["cargo", "test", "-p", package]
        if lib:
            test_command.append("--lib")
        elif integration_test:
            test_command.extend(("--test", integration_test))
        test_command.append("--locked")
        if test_filter:
            test_command.append(test_filter)
        steps.append(Step(f"Test {package}", tuple(test_command)))

    return tuple(steps)


def full_plan() -> tuple[Step, ...]:
    """Plan the full local equivalent of workspace quality checks."""
    return (
        Step("Check formatting", ("cargo", "fmt", "--all", "--", "--check")),
        Step(
            "Check architecture guardrails",
            ("python3", "scripts/check_architecture.py"),
        ),
        Step(
            "Test architecture guardrails",
            ("python3", "scripts/check_architecture.py", "--self-test"),
        ),
        Step(
            "Check release versions",
            ("python3", "scripts/check_release_versions.py"),
        ),
        Step(
            "Check SDK compatibility metadata",
            ("python3", "scripts/check_sdk_compatibility.py"),
        ),
        Step(
            "Test SDK redaction evidence schema",
            ("python3", "scripts/audit_sdk_redaction_tests.py"),
        ),
        Step("Test shell installer", ("sh", "scripts/install_tests.sh")),
        Step(
            "Test validation scripts",
            (
                "python3",
                "-m",
                "unittest",
                "discover",
                "-s",
                "scripts/tests",
                "-p",
                "test_*.py",
            ),
        ),
        Step(
            "Run Clippy",
            (
                "cargo",
                "clippy",
                "--workspace",
                "--all-targets",
                "--all-features",
                "--locked",
                "--",
                "-D",
                "warnings",
            ),
        ),
        Step(
            "Run workspace tests",
            ("cargo", "test", "--workspace", "--locked"),
        ),
        # Must directly follow the workspace tests: they refresh
        # target/debug/rho, and the script's own build would recompile
        # rho-coding-agent (see --bin in check_docs_ui_demo.sh).
        Step(
            "Check docs TUI proof plate",
            (
                "bash",
                "scripts/check_docs_ui_demo.sh",
                "--check",
                "--bin",
                "target/debug/rho",
            ),
        ),
        # Every other cargo step enables workspace dev-dependency features, so
        # this is the only local check that rho compiles as `cargo install`
        # and release builds see it.
        Step(
            "Check rho without dev-dependency features",
            ("cargo", "check", "-p", "rho-coding-agent", "--bin", "rho", "--locked"),
        ),
        Step(
            "Run documentation tests",
            (
                "cargo",
                "test",
                "--workspace",
                "--doc",
                "--all-features",
                "--locked",
            ),
        ),
        Step(
            "Test SDK feature modes",
            ("python3", "scripts/check_sdk_compatibility.py", "--test-features"),
        ),
        Step(
            "Check downstream SDK fixtures",
            ("python3", "scripts/check_sdk_compatibility.py", "--test-downstream"),
        ),
    )


def capped_cargo_jobs(value: str | None, *, cpu_count: int | None = None) -> str:
    """Cap Cargo jobs while honoring a lower positive or relative setting."""
    if value is None:
        return str(MAX_CARGO_JOBS)

    try:
        requested = int(value)
    except ValueError as error:
        raise ValueError(f"CARGO_BUILD_JOBS must be an integer, got {value!r}") from error

    if requested == 0:
        raise ValueError("CARGO_BUILD_JOBS must not be zero")
    if requested < 0:
        available = cpu_count if cpu_count is not None else (os.cpu_count() or 1)
        requested = max(1, available + requested)
    return str(min(MAX_CARGO_JOBS, requested))


def rustc_min_stack(value: str | None) -> str:
    """Keep rustc's compile-thread stack at or above the 1.92 ICE floor."""
    if value is None or not value.strip():
        return str(RUSTC_MIN_STACK_BYTES)
    try:
        requested = int(value)
    except ValueError as error:
        raise ValueError(f"RUST_MIN_STACK must be an integer, got {value!r}") from error
    if requested < RUSTC_MIN_STACK_BYTES:
        return str(RUSTC_MIN_STACK_BYTES)
    return str(requested)


def run_plan(steps: Sequence[Step]) -> None:
    """Run steps in order, stop at the first failure, and report step timings."""
    environment = os.environ.copy()
    environment["CARGO_BUILD_JOBS"] = capped_cargo_jobs(
        environment.get("CARGO_BUILD_JOBS")
    )
    environment["RUST_MIN_STACK"] = rustc_min_stack(
        environment.get("RUST_MIN_STACK")
    )
    print(f"Cargo jobs: {environment['CARGO_BUILD_JOBS']}", flush=True)

    timings: list[tuple[str, float]] = []
    try:
        for step in steps:
            print(f"\n==> {step.label}", flush=True)
            print(f"+ {' '.join(step.command)}", flush=True)
            started = time.monotonic()
            try:
                subprocess.run(step.command, cwd=ROOT, env=environment, check=True)
            finally:
                timings.append((step.label, time.monotonic() - started))
    finally:
        print("\n==> Step timings", flush=True)
        for label, seconds in timings:
            print(f"{seconds:8.1f}s  {label}", flush=True)
        print(f"{sum(s for _, s in timings):8.1f}s  total", flush=True)


def parse_args(arguments: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_subparsers(dest="mode", required=True)

    fast = modes.add_parser("fast", help="check one package and optional tests")
    fast.add_argument("--package", "-p", required=True, help="Cargo package name")
    target = fast.add_mutually_exclusive_group()
    target.add_argument("--lib", action="store_true", help="run library tests")
    target.add_argument("--test", dest="integration_test", help="integration test target")
    fast.add_argument("--filter", dest="test_filter", help="Cargo test name filter")

    modes.add_parser("full", help="run all workspace quality checks")
    return parser.parse_args(arguments)


def main(arguments: Sequence[str] | None = None) -> int:
    options = parse_args(arguments)
    if options.mode == "fast":
        plan = fast_plan(
            options.package,
            lib=options.lib,
            integration_test=options.integration_test,
            test_filter=options.test_filter,
        )
    else:
        plan = full_plan()

    run_plan(plan)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (ValueError, subprocess.CalledProcessError) as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(1)
