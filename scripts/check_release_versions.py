#!/usr/bin/env python3
"""Ensure Release Please and Cargo agree on independently released versions.

Cargo may sit exactly one unpublished patch, minor, or major ahead of the Release
Please manifest. Publish dry-run path-patches unpublished versions so a new public
API can land before the next tag. The manifest must stay on the last tagged version;
if it is pre-bumped without a tag, Release Please loses the baseline and can
rewrite history as a false major.
"""

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
WORKSPACE_MANIFEST = ROOT / "Cargo.toml"


def load_json(path: Path) -> dict[str, object]:
    return json.loads(path.read_text(encoding="utf-8"))


def parse_semver(version: str) -> tuple[int, int, int]:
    """Parse a `major.minor.patch` version used by Cargo and Release Please."""
    parts = version.split(".")
    if len(parts) != 3 or not all(part.isdigit() for part in parts):
        raise ValueError(f"expected major.minor.patch, got {version!r}")
    return int(parts[0]), int(parts[1]), int(parts[2])


def cargo_agrees_with_release_baseline(cargo_version: str, release_version: str) -> bool:
    """Return whether Cargo matches or is the next unpublished patch/minor."""
    if cargo_version == release_version:
        return True
    try:
        cargo = parse_semver(cargo_version)
        released = parse_semver(release_version)
    except ValueError:
        return False
    major, minor, patch = released
    return cargo in {
        (major, minor, patch + 1),
        (major, minor + 1, 0),
        (major + 1, 0, 0),
    }


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


def load_toml(path: Path) -> dict[str, object]:
    with path.open("rb") as file:
        return tomllib.load(file)


def workspace_member_manifests() -> list[Path]:
    """Return Cargo manifests for root workspace members, including unpublished crates."""
    workspace = load_toml(WORKSPACE_MANIFEST).get("workspace")
    if not isinstance(workspace, dict):
        raise RuntimeError("root Cargo.toml is not a workspace")
    members = workspace.get("members")
    if not isinstance(members, list) or not all(
        isinstance(member, str) for member in members
    ):
        raise RuntimeError("workspace.members must be an array of strings")
    manifests = []
    for member in members:
        manifest = ROOT / member / "Cargo.toml"
        if not manifest.is_file():
            raise RuntimeError(f"workspace member {member!r} is missing Cargo.toml")
        manifests.append(manifest)
    return manifests


def iter_manifest_dependency_names(manifest: dict[str, object]) -> set[str]:
    """Return dependency table keys, matching release-please's cargo-workspace plugin."""
    names: set[str] = set()

    def take(table: object) -> None:
        if isinstance(table, dict):
            names.update(str(key) for key in table)

    for table_name in DEPENDENCY_TABLES:
        take(manifest.get(table_name))
    targets = manifest.get("target")
    if isinstance(targets, dict):
        for target in targets.values():
            if not isinstance(target, dict):
                continue
            for table_name in DEPENDENCY_TABLES:
                take(target.get(table_name))
    return names


def workspace_package_graph() -> dict[str, set[str]]:
    """Map each workspace package name to other workspace packages it depends on."""
    crates: dict[str, set[str]] = {}
    for manifest_path in workspace_member_manifests():
        manifest = load_toml(manifest_path)
        package = manifest.get("package")
        if not isinstance(package, dict) or not isinstance(package.get("name"), str):
            relative = manifest_path.relative_to(ROOT)
            raise RuntimeError(f"{relative} is missing [package.name]")
        crates[package["name"]] = iter_manifest_dependency_names(manifest)
    workspace_names = set(crates)
    return {
        name: deps & workspace_names
        for name, deps in crates.items()
    }


def find_workspace_dependency_cycle(
    graph: dict[str, set[str]],
) -> tuple[str, ...] | None:
    """Return one directed cycle among workspace packages, if any.

    Release Please's cargo-workspace plugin walks this graph, including
    dev-dependencies, and fails closed on a cycle.
    """
    visiting: set[str] = set()
    visited: set[str] = set()
    stack: list[str] = []

    def visit(name: str) -> tuple[str, ...] | None:
        if name in visited:
            return None
        if name in visiting:
            start = stack.index(name)
            return tuple(stack[start:] + [name])
        visiting.add(name)
        stack.append(name)
        for dep in sorted(graph.get(name, ())):
            cycle = visit(dep)
            if cycle is not None:
                return cycle
        stack.pop()
        visiting.remove(name)
        visited.add(name)
        return None

    for name in sorted(graph):
        cycle = visit(name)
        if cycle is not None:
            return cycle
    return None


def check_workspace_dependency_graph() -> None:
    cycle = find_workspace_dependency_cycle(workspace_package_graph())
    if cycle is None:
        return
    raise RuntimeError(
        "found cycle in dependency graph: "
        + " -> ".join(cycle)
        + "; release-please's cargo-workspace plugin cannot order this workspace"
    )


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
        if not isinstance(release_version, str):
            raise RuntimeError(
                f"{release_path} release-please manifest version must be a string"
            )
        if not cargo_agrees_with_release_baseline(cargo_version, release_version):
            raise RuntimeError(
                f"{release_path} Cargo version {cargo_version} does not match "
                f"release-please manifest version {release_version}. "
                "Cargo may be exactly one unpublished patch, minor, or major ahead "
                "of the last tagged Release Please version."
            )

    check_internal_dependency_versions()
    check_workspace_dependency_graph()
    print("Release Please, Cargo package, and internal dependency versions are consistent")


if __name__ == "__main__":
    main()
