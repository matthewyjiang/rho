#!/usr/bin/env python3
"""Require pull requests that change released crates to trigger a version bump."""

from __future__ import annotations

import argparse
import re
import subprocess
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PACKAGES = {
    "crates/rho": "rho-coding-agent",
    "crates/rho-providers": "rho-providers",
    "crates/rho-sdk": "rho-sdk",
    "crates/rho-tools": "rho-agent-tools",
}
RELEASE_TITLE = re.compile(r"^(?:feat|fix)(?:\([^)]*\))?!?:|^[a-z]+(?:\([^)]*\))?!:")


def run_git(*args: str) -> str:
    return subprocess.run(
        ["git", *args],
        cwd=ROOT,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
    ).stdout


def manifest_version(contents: bytes) -> str:
    return tomllib.loads(contents.decode())["package"]["version"]


def version_at(revision: str, package_path: str) -> str:
    contents = subprocess.run(
        ["git", "show", f"{revision}:{package_path}/Cargo.toml"],
        cwd=ROOT,
        check=True,
        stdout=subprocess.PIPE,
    ).stdout
    return manifest_version(contents)


def title_triggers_release(title: str) -> bool:
    return RELEASE_TITLE.match(title) is not None


def changed_packages(base: str) -> list[str]:
    changed = run_git("diff", "--name-only", f"{base}...HEAD").splitlines()
    return [
        package_path
        for package_path in PACKAGES
        if any(path == package_path or path.startswith(f"{package_path}/") for path in changed)
    ]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--base", required=True, help="pull request base commit")
    parser.add_argument("--title", required=True, help="pull request title")
    args = parser.parse_args()

    failures = []
    for package_path in changed_packages(args.base):
        current_manifest = ROOT / package_path / "Cargo.toml"
        with current_manifest.open("rb") as file:
            current_version = manifest_version(file.read())
        base_version = version_at(args.base, package_path)
        if current_version == base_version and not title_triggers_release(args.title):
            failures.append(f"{PACKAGES[package_path]} remains at released version {current_version}")

    if failures:
        details = "\n".join(f"- {failure}" for failure in failures)
        raise RuntimeError(
            "This pull request changes publishable crates but its title will not make "
            "Release Please assign new versions:\n"
            f"{details}\n"
            "Use a fix: or feat: title, mark a breaking change with !, or bump the "
            "affected package versions."
        )

    print("Changed publishable crates will receive new release versions")


if __name__ == "__main__":
    main()
