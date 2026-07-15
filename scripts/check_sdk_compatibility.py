#!/usr/bin/env python3
"""Enforce rho-sdk feature, MSRV, deprecation, and downstream contracts."""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SDK_MANIFEST = ROOT / "crates" / "rho-sdk" / "Cargo.toml"
DOWNSTREAM_ROOT = ROOT / "fixtures" / "downstream"


def load_toml(path: Path) -> dict:
    with path.open("rb") as file:
        return tomllib.load(file)


def run(*arguments: str, cwd: Path = ROOT) -> None:
    print(f"+ {' '.join(arguments)}", flush=True)
    subprocess.run(arguments, cwd=cwd, check=True)


def check_metadata() -> None:
    sdk = load_toml(SDK_MANIFEST)
    application = load_toml(ROOT / "crates" / "rho" / "Cargo.toml")

    if sdk["features"].get("default") != []:
        raise RuntimeError("rho-sdk default features must remain empty")

    sdk_msrv = sdk["package"].get("rust-version")
    application_msrv = application["package"].get("rust-version")
    if not sdk_msrv:
        raise RuntimeError("rho-sdk must declare package.rust-version")
    if not application_msrv:
        raise RuntimeError("rho-coding-agent must declare package.rust-version")

    policy = (ROOT / "docs" / "sdk" / "compatibility.md").read_text(encoding="utf-8")
    policy_markers = {
        "rho-sdk": f"`rho-sdk` minimum supported Rust version (MSRV) is **{sdk_msrv}**",
        "rho-coding-agent": (
            f"`rho-coding-agent` application MSRV is **{application_msrv}**"
        ),
    }
    for name, marker in policy_markers.items():
        if marker not in policy:
            raise RuntimeError(f"compatibility policy must document {name} MSRV")
    fixture_workspace = load_toml(DOWNSTREAM_ROOT / "Cargo.toml")
    for member in fixture_workspace["workspace"]["members"]:
        manifest = load_toml(DOWNSTREAM_ROOT / member / "Cargo.toml")
        dependency_tables = [
            manifest.get("dependencies", {}),
            manifest.get("dev-dependencies", {}),
            manifest.get("build-dependencies", {}),
        ]
        for target in manifest.get("target", {}).values():
            dependency_tables.extend(
                (
                    target.get("dependencies", {}),
                    target.get("dev-dependencies", {}),
                    target.get("build-dependencies", {}),
                )
            )
        dependency_names = {
            name for table in dependency_tables for name in table.keys()
        }
        if dependency_names != {"rho-sdk"}:
            raise RuntimeError(
                f"downstream fixture {member} must depend only on rho-sdk"
            )
        dependency = manifest["dependencies"]["rho-sdk"]
        expected = (DOWNSTREAM_ROOT / member / dependency["path"]).resolve()
        if expected != SDK_MANIFEST.parent.resolve():
            raise RuntimeError(
                f"downstream fixture {member} must use the local rho-sdk"
            )

    deprecated = re.compile(r"#\s*\[\s*deprecated(?P<arguments>[^\]]*)\]", re.DOTALL)
    for source in (SDK_MANIFEST.parent / "src").rglob("*.rs"):
        text = source.read_text(encoding="utf-8")
        for match in deprecated.finditer(text):
            arguments = match.group("arguments")
            if not re.search(r"\bsince\s*=", arguments) or not re.search(
                r"\bnote\s*=", arguments
            ):
                relative = source.relative_to(ROOT)
                raise RuntimeError(
                    f"{relative} has #[deprecated] without both since and note"
                )

    print("rho-sdk compatibility metadata is valid")


def test_features() -> None:
    modes = (
        ("default features", []),
        ("no default features", ["--no-default-features"]),
        ("all features", ["--all-features"]),
    )
    for label, flags in modes:
        print(f"Testing rho-sdk with {label}", flush=True)
        run(
            "cargo",
            "test",
            "-p",
            "rho-sdk",
            "--all-targets",
            "--locked",
            *flags,
        )


def test_downstream() -> None:
    run(
        "cargo",
        "fmt",
        "--all",
        "--",
        "--check",
        cwd=DOWNSTREAM_ROOT,
    )
    run(
        "cargo",
        "check",
        "--workspace",
        "--all-targets",
        "--locked",
        cwd=DOWNSTREAM_ROOT,
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test-features", action="store_true")
    parser.add_argument("--test-downstream", action="store_true")
    arguments = parser.parse_args()

    check_metadata()
    if arguments.test_features:
        test_features()
    if arguments.test_downstream:
        test_downstream()
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (KeyError, OSError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"error: {error}", file=sys.stderr)
        sys.exit(1)
