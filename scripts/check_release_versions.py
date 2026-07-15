#!/usr/bin/env python3
"""Ensure Release Please and Cargo agree on independently released versions."""

from __future__ import annotations

import json
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PACKAGES = {
    "crates/rho": ROOT / "crates" / "rho" / "Cargo.toml",
    "crates/rho-sdk": ROOT / "crates" / "rho-sdk" / "Cargo.toml",
}


def load_json(path: Path) -> dict[str, object]:
    return json.loads(path.read_text(encoding="utf-8"))


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

    print("Release Please and Cargo package versions are consistent")


if __name__ == "__main__":
    main()
