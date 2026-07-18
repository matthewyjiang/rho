#!/usr/bin/env python3
"""Ensure Release Please and Cargo agree on independently released versions."""

from __future__ import annotations

import json
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PACKAGES = {
    "crates/rho": ROOT / "crates" / "rho" / "Cargo.toml",
    "crates/rho-providers": ROOT / "crates" / "rho-providers" / "Cargo.toml",
    "crates/rho-sdk": ROOT / "crates" / "rho-sdk" / "Cargo.toml",
    "crates/rho-tools": ROOT / "crates" / "rho-tools" / "Cargo.toml",
}


def load_json(path: Path) -> dict[str, object]:
    return json.loads(path.read_text(encoding="utf-8"))


def check_internal_dependency_versions() -> None:
    package_versions: dict[Path, str] = {}
    for cargo_manifest in PACKAGES.values():
        with cargo_manifest.open("rb") as file:
            package_versions[cargo_manifest.resolve()] = tomllib.load(file)["package"]["version"]

    dependency_tables = ("dependencies", "dev-dependencies", "build-dependencies")
    for cargo_manifest in PACKAGES.values():
        with cargo_manifest.open("rb") as file:
            manifest = tomllib.load(file)
        for table_name in dependency_tables:
            for dependency_name, dependency in manifest.get(table_name, {}).items():
                if not isinstance(dependency, dict) or "path" not in dependency:
                    continue
                dependency_manifest = (
                    cargo_manifest.parent / dependency["path"] / "Cargo.toml"
                ).resolve()
                expected_version = package_versions.get(dependency_manifest)
                if expected_version is None:
                    continue
                actual_version = dependency.get("version")
                if actual_version != expected_version:
                    raise RuntimeError(
                        f"{cargo_manifest.relative_to(ROOT)} {dependency_name} dependency "
                        f"requires {actual_version!r}, but the workspace package version is "
                        f"{expected_version}"
                    )


def main() -> None:
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
