#!/usr/bin/env python3
"""Run the common local validation workflows."""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence

ROOT = Path(__file__).resolve().parents[1]
MAX_CARGO_JOBS = 12


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
        Step(
            "Check docs TUI proof plate",
            ("bash", "scripts/check_docs_ui_demo.sh", "--check"),
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


def run_plan(steps: Sequence[Step]) -> None:
    """Run steps in order and stop at the first failure."""
    environment = os.environ.copy()
    environment["CARGO_BUILD_JOBS"] = capped_cargo_jobs(
        environment.get("CARGO_BUILD_JOBS")
    )
    print(f"Cargo jobs: {environment['CARGO_BUILD_JOBS']}", flush=True)

    for step in steps:
        print(f"\n==> {step.label}", flush=True)
        print(f"+ {' '.join(step.command)}", flush=True)
        subprocess.run(step.command, cwd=ROOT, env=environment, check=True)


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
