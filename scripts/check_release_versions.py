#!/usr/bin/env python3
"""Ensure Release Please and Cargo agree on independently released versions."""

from __future__ import annotations

import argparse
import json
import re
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PACKAGES = {
    "crates/rho": ROOT / "crates" / "rho" / "Cargo.toml",
    "crates/rho-providers": ROOT / "crates" / "rho-providers" / "Cargo.toml",
    "crates/rho-sdk": ROOT / "crates" / "rho-sdk" / "Cargo.toml",
    "crates/rho-tools": ROOT / "crates" / "rho-tools" / "Cargo.toml",
}
DEPENDENCY_TABLES = ("dependencies", "dev-dependencies", "build-dependencies")


def load_json(path: Path) -> dict[str, object]:
    return json.loads(path.read_text(encoding="utf-8"))


def package_versions() -> dict[Path, str]:
    versions: dict[Path, str] = {}
    for cargo_manifest in PACKAGES.values():
        with cargo_manifest.open("rb") as file:
            versions[cargo_manifest.resolve()] = tomllib.load(file)["package"]["version"]
    return versions


def iter_internal_dependency_mismatches(
    versions: dict[Path, str],
) -> list[tuple[Path, str, str, str]]:
    """Return (manifest, dependency_name, actual_version, expected_version)."""
    mismatches: list[tuple[Path, str, str, str]] = []
    for cargo_manifest in PACKAGES.values():
        with cargo_manifest.open("rb") as file:
            manifest = tomllib.load(file)
        for table_name in DEPENDENCY_TABLES:
            for dependency_name, dependency in manifest.get(table_name, {}).items():
                if not isinstance(dependency, dict) or "path" not in dependency:
                    continue
                dependency_manifest = (
                    cargo_manifest.parent / dependency["path"] / "Cargo.toml"
                ).resolve()
                expected_version = versions.get(dependency_manifest)
                if expected_version is None:
                    continue
                actual_version = dependency.get("version")
                if actual_version != expected_version:
                    mismatches.append(
                        (
                            cargo_manifest,
                            dependency_name,
                            str(actual_version),
                            expected_version,
                        )
                    )
    return mismatches


def sync_internal_dependency_versions() -> list[Path]:
    """Align path dependency versions with workspace package versions.

    Release Please's cargo-workspace plugin matches dependency table keys to
    package names, so renamed path deps like `rho-tools` / package =
    `rho-agent-tools` are left stale. Rewrite those pins in place.
    """
    versions = package_versions()
    mismatches = iter_internal_dependency_mismatches(versions)
    changed: list[Path] = []
    updates_by_manifest: dict[Path, list[tuple[str, str, str]]] = {}
    for cargo_manifest, dependency_name, actual_version, expected_version in mismatches:
        updates_by_manifest.setdefault(cargo_manifest, []).append(
            (dependency_name, actual_version, expected_version)
        )

    for cargo_manifest, updates in updates_by_manifest.items():
        original = cargo_manifest.read_text(encoding="utf-8")
        updated = original
        for dependency_name, actual_version, expected_version in updates:
            pattern = re.compile(
                rf"^({re.escape(dependency_name)}\s*=\s*\{{[^\n]*?\bversion\s*=\s*\")"
                rf"{re.escape(actual_version)}"
                rf'(")',
                re.MULTILINE,
            )
            updated, count = pattern.subn(
                rf"\g<1>{expected_version}\2", updated, count=1
            )
            if count != 1:
                raise RuntimeError(
                    f"failed to rewrite {dependency_name} version "
                    f"{actual_version!r} -> {expected_version!r} in "
                    f"{cargo_manifest.relative_to(ROOT)}"
                )
        if updated != original:
            cargo_manifest.write_text(updated, encoding="utf-8")
            changed.append(cargo_manifest)

    return changed


def check_internal_dependency_versions() -> None:
    mismatches = iter_internal_dependency_mismatches(package_versions())
    if not mismatches:
        return
    cargo_manifest, dependency_name, actual_version, expected_version = mismatches[0]
    raise RuntimeError(
        f"{cargo_manifest.relative_to(ROOT)} {dependency_name} dependency "
        f"requires {actual_version!r}, but the workspace package version is "
        f"{expected_version}"
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--fix-internal-deps",
        action="store_true",
        help=(
            "Rewrite stale workspace path dependency versions before validating. "
            "Use on Release Please PR branches after package versions bump."
        ),
    )
    args = parser.parse_args()

    if args.fix_internal_deps:
        changed = sync_internal_dependency_versions()
        if changed:
            relative = ", ".join(str(path.relative_to(ROOT)) for path in changed)
            print(f"Synced internal dependency versions in {relative}")
        else:
            print("Internal dependency versions already matched package versions")

    config = load_json(ROOT / ".release-please-config.json")
    manifest = load_json(ROOT / ".release-please-manifest.json")
    configured_paths = set(config["packages"])
    expected_paths = set(PACKAGES)

    if configured_paths != expected_paths:
        raise RuntimeError(
            "release-please package paths differ from the independently released "
            f"Cargo packages: configured={sorted(configured_paths)}, "
            f"expected={sorted(expected_paths)}"
        )
    if set(manifest) != expected_paths:
        raise RuntimeError(
            "release-please manifest paths differ from the independently released "
            f"Cargo packages: manifest={sorted(manifest)}, "
            f"expected={sorted(expected_paths)}"
        )

    for release_path, cargo_manifest in PACKAGES.items():
        with cargo_manifest.open("rb") as file:
            cargo_version = tomllib.load(file)["package"]["version"]
        release_version = manifest[release_path]
        if cargo_version != release_version:
            raise RuntimeError(
                f"{release_path} Cargo version {cargo_version} does not match "
                f"release-please manifest version {release_version}"
            )

    check_internal_dependency_versions()
    print("Release Please, Cargo package, and internal dependency versions are consistent")


if __name__ == "__main__":
    main()
