#!/usr/bin/env python3
"""Decide crates.io path patches for workspace publish verification.

Release prep must not hide registry-boundary breaks behind workspace path
patches. Rule:

- If every internal dependency's exact workspace version already exists on
  crates.io (including yanked releases), verify against the registry. Do not
  path-patch any dependency.
- If any direct internal dependency version is not on crates.io yet, path-patch
  the full transitive internal dependency closure of the package under test.
  Path-source crates keep `path =` edges to siblings; patching only the
  unpublished leaf would load a second copy of shared crates such as
  `rho-sdk` and break type identity. The closure patch keeps one graph so a
  coordinated same-PR cut can still package before dependencies are published.
- crates.io transport or HTTP failures fail the check. A timeout or 500 is not
  treated as "unpublished".

Used by scripts/check_crate_publish_prep.sh and the dry-run path in
scripts/publish_workspace_crates.sh.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Sequence
from urllib.parse import quote

ROOT = Path(__file__).resolve().parents[1]
CRATES_IO_UA = "rho-crate-publish-prep/1.0 (https://github.com/matthewyjiang/rho)"
PUBLISH_CRATES = (
    "rho-sdk",
    "rho-agent-tools",
    "rho-providers",
    "rho-coding-agent",
)
INTERNAL_PACKAGE_NAMES = frozenset(PUBLISH_CRATES)
DEFAULT_FIXTURE_ROOT = ROOT / "fixtures" / "publish-boundary"


@dataclass(frozen=True)
class WorkspacePackage:
    """One independently released workspace package."""

    name: str
    version: str
    manifest_path: Path
    package_root: Path


@dataclass(frozen=True)
class InternalDependency:
    """A path dependency on another released workspace package."""

    package_name: str
    version: str
    package_root: Path


@dataclass(frozen=True)
class PathPatch:
    """A cargo --config patch.crates-io entry."""

    package_name: str
    path: Path

    def cargo_config_flag(self, *, relative_to: Path = ROOT) -> str:
        rel = self.path.resolve().relative_to(relative_to.resolve()).as_posix()
        return f'patch.crates-io.{self.package_name}.path="{rel}"'


class RegistryError(RuntimeError):
    """crates.io could not be queried reliably."""


VersionProbe = Callable[[str, str], bool]


def run(*arguments: str, cwd: Path = ROOT) -> None:
    print(f"+ {' '.join(arguments)}", flush=True)
    subprocess.run(arguments, cwd=cwd, check=True)


def load_metadata(*, root: Path = ROOT) -> dict[str, object]:
    payload = subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        cwd=root,
        text=True,
    )
    return json.loads(payload)


def load_workspace_packages(
    metadata: dict[str, object],
) -> dict[str, WorkspacePackage]:
    """Return released workspace packages keyed by package name."""
    packages: dict[str, WorkspacePackage] = {}
    for raw in metadata.get("packages", []):
        if not isinstance(raw, dict):
            continue
        name = raw.get("name")
        if name not in INTERNAL_PACKAGE_NAMES:
            continue
        version = raw.get("version")
        manifest = raw.get("manifest_path")
        if not isinstance(version, str) or not isinstance(manifest, str):
            raise RuntimeError(f"incomplete cargo metadata for {name!r}")
        manifest_path = Path(manifest)
        packages[name] = WorkspacePackage(
            name=name,
            version=version,
            manifest_path=manifest_path,
            package_root=manifest_path.parent,
        )
    missing = INTERNAL_PACKAGE_NAMES.difference(packages)
    if missing:
        raise RuntimeError(
            "cargo metadata is missing released packages: "
            + ", ".join(sorted(missing))
        )
    return packages


def internal_dependencies_for(
    package_name: str,
    metadata: dict[str, object],
) -> tuple[InternalDependency, ...]:
    """Return internal path deps for package_name (normal and build only)."""
    packages = load_workspace_packages(metadata)
    target = None
    for raw in metadata.get("packages", []):
        if isinstance(raw, dict) and raw.get("name") == package_name:
            target = raw
            break
    if target is None:
        raise RuntimeError(f"package {package_name!r} not found in cargo metadata")

    deps: list[InternalDependency] = []
    seen: set[str] = set()
    for raw_dep in target.get("dependencies", []):
        if not isinstance(raw_dep, dict):
            continue
        kind = raw_dep.get("kind")
        if kind not in (None, "normal", "build"):
            continue
        dep_name = raw_dep.get("name")
        if dep_name not in INTERNAL_PACKAGE_NAMES:
            continue
        dep_path = raw_dep.get("path")
        if not isinstance(dep_path, str):
            continue
        if dep_name in seen:
            continue
        seen.add(dep_name)
        workspace_dep = packages[dep_name]
        deps.append(
            InternalDependency(
                package_name=dep_name,
                version=workspace_dep.version,
                package_root=Path(dep_path),
            )
        )
    deps.sort(key=lambda item: item.package_name)
    return tuple(deps)


def crates_io_version_available(
    package_name: str,
    version: str,
    *,
    opener: Callable[..., object] = urllib.request.urlopen,
    timeout_seconds: float = 20.0,
) -> bool:
    """Return True when crates.io has the exact version document.

    Yanked releases still count as present: publish verification should see the
    registry boundary rather than silently path-patching over it. 404 means the
    version is absent. Any other HTTP or transport failure raises RegistryError
    so callers fail closed.
    """
    url = (
        "https://crates.io/api/v1/crates/"
        f"{quote(package_name, safe='')}/{quote(version, safe='')}"
    )
    request = urllib.request.Request(url, headers={"User-Agent": CRATES_IO_UA})
    try:
        with opener(request, timeout=timeout_seconds) as response:
            status = getattr(response, "status", None)
            if status is None:
                status = response.getcode()
            if status != 200:
                raise RegistryError(
                    f"crates.io returned HTTP {status} for {package_name}@{version}"
                )
            # Consume the body so keep-alive connections close cleanly.
            response.read()
    except urllib.error.HTTPError as error:
        try:
            if error.code == 404:
                return False
            raise RegistryError(
                f"crates.io HTTP {error.code} for {package_name}@{version}: "
                f"{error.reason}"
            ) from error
        finally:
            error.close()
    except urllib.error.URLError as error:
        raise RegistryError(
            f"crates.io request failed for {package_name}@{version}: {error.reason}"
        ) from error
    except TimeoutError as error:
        raise RegistryError(
            f"crates.io request timed out for {package_name}@{version}"
        ) from error

    return True


def path_patches_for_dependencies(
    dependencies: Sequence[InternalDependency],
    *,
    version_available: VersionProbe,
    cache: dict[tuple[str, str], bool] | None = None,
) -> tuple[PathPatch, ...]:
    """Choose path patches for dependencies from an availability probe."""
    availability = cache if cache is not None else {}
    patches: list[PathPatch] = []
    for dep in dependencies:
        key = (dep.package_name, dep.version)
        if key not in availability:
            availability[key] = version_available(dep.package_name, dep.version)
        if availability[key]:
            continue
        patches.append(
            PathPatch(package_name=dep.package_name, path=dep.package_root)
        )
    return tuple(patches)


def internal_dependency_closure(
    package_name: str,
    metadata: dict[str, object],
) -> tuple[InternalDependency, ...]:
    """Return every transitive internal path dep of package_name, sorted."""
    packages = load_workspace_packages(metadata)
    adjacency: dict[str, tuple[InternalDependency, ...]] = {}
    for name in INTERNAL_PACKAGE_NAMES:
        adjacency[name] = internal_dependencies_for(name, metadata)

    ordered: list[InternalDependency] = []
    seen: set[str] = set()
    stack = list(reversed(adjacency.get(package_name, ())))
    while stack:
        dep = stack.pop()
        if dep.package_name in seen:
            continue
        seen.add(dep.package_name)
        # Prefer the workspace package root so patches stay stable even if a
        # dependent lists a slightly different path spelling.
        workspace = packages[dep.package_name]
        ordered.append(
            InternalDependency(
                package_name=dep.package_name,
                version=workspace.version,
                package_root=workspace.package_root,
            )
        )
        for child in reversed(adjacency.get(dep.package_name, ())):
            if child.package_name not in seen:
                stack.append(child)
    ordered.sort(key=lambda item: item.package_name)
    return tuple(ordered)


def select_path_patches(
    package_name: str,
    *,
    metadata: dict[str, object] | None = None,
    root: Path = ROOT,
    version_available: VersionProbe | None = None,
    cache: dict[tuple[str, str], bool] | None = None,
) -> tuple[PathPatch, ...]:
    """Return path patches required to verify package_name for publish.

    Published-only graphs stay on crates.io so missing exports fail closed.
    When any direct internal dep is unpublished, path-patch the full internal
    closure so path-source crates share one type identity with the consumer.
    """
    if metadata is None:
        metadata = load_metadata(root=root)
    probe = version_available or crates_io_version_available
    direct = internal_dependencies_for(package_name, metadata)
    unpublished = path_patches_for_dependencies(
        direct,
        version_available=probe,
        cache=cache,
    )
    if not unpublished:
        return ()
    closure = internal_dependency_closure(package_name, metadata)
    return tuple(
        PathPatch(package_name=dep.package_name, path=dep.package_root)
        for dep in closure
    )


def cargo_config_flags(
    patches: Sequence[PathPatch], *, relative_to: Path = ROOT
) -> tuple[str, ...]:
    """Expand path patches into cargo --config flag pairs."""
    flags: list[str] = []
    for patch in patches:
        flags.extend(("--config", patch.cargo_config_flag(relative_to=relative_to)))
    return tuple(flags)


def describe_patch_plan(
    package_name: str, patches: Sequence[PathPatch]
) -> str:
    if not patches:
        return (
            f"{package_name}: verify internal deps against crates.io "
            "(no path patches)"
        )
    rendered = ", ".join(
        f"{patch.package_name} -> {patch.path}" for patch in patches
    )
    return (
        f"{package_name}: path-patch unpublished same-cut deps: {rendered}"
    )


def verify_package(
    package_name: str,
    *,
    metadata: dict[str, object] | None = None,
    root: Path = ROOT,
    version_available: VersionProbe | None = None,
    cache: dict[tuple[str, str], bool] | None = None,
    runner: Callable[..., None] = run,
) -> None:
    """Run cargo publish --dry-run for one package with selective patches."""
    patches = select_path_patches(
        package_name,
        metadata=metadata,
        root=root,
        version_available=version_available,
        cache=cache,
    )
    print(describe_patch_plan(package_name, patches), flush=True)
    command = [
        "cargo",
        "publish",
        "--dry-run",
        "--locked",
        "-p",
        package_name,
        *cargo_config_flags(patches, relative_to=root),
    ]
    runner(*command, cwd=root)


def verify_all_packages(
    *,
    root: Path = ROOT,
    version_available: VersionProbe | None = None,
    runner: Callable[..., None] = run,
) -> None:
    """Verify every independently released workspace crate."""
    metadata = load_metadata(root=root)
    cache: dict[tuple[str, str], bool] = {}
    for package_name in PUBLISH_CRATES:
        verify_package(
            package_name,
            metadata=metadata,
            root=root,
            version_available=version_available,
            cache=cache,
            runner=runner,
        )
    print("Crate publish preparation checks passed", flush=True)


def print_cargo_config_flags(package_name: str, *, root: Path = ROOT) -> int:
    """Print cargo --config flags for package_name, one token per line."""
    patches = select_path_patches(package_name, root=root)
    print(describe_patch_plan(package_name, patches), file=sys.stderr, flush=True)
    for flag in cargo_config_flags(patches, relative_to=root):
        print(flag)
    return 0


def run_boundary_fixture(fixture_root: Path = DEFAULT_FIXTURE_ROOT) -> None:
    """Prove missing registry exports fail without a workspace path patch.

    Layout under fixture_root:

    - registry/boundary-dep: published-shaped API without EXTRA_SYMBOL
    - workspace/boundary-dep: same version with EXTRA_SYMBOL
    - consumer: depends on boundary-dep = \"=0.1.0\" and imports EXTRA_SYMBOL
    """
    registry_dep = (fixture_root / "registry" / "boundary-dep").resolve()
    workspace_dep = (fixture_root / "workspace" / "boundary-dep").resolve()
    consumer_manifest = (fixture_root / "consumer" / "Cargo.toml").resolve()

    for required in (
        registry_dep / "Cargo.toml",
        workspace_dep / "Cargo.toml",
        consumer_manifest,
    ):
        if not required.is_file():
            raise FileNotFoundError(f"publish-boundary fixture missing {required}")

    def flag_for(dep_path: Path) -> str:
        rel = dep_path.resolve().relative_to(fixture_root.resolve()).as_posix()
        return f'patch.crates-io.boundary-dep.path="{rel}"'

    def run_check(dep_path: Path) -> subprocess.CompletedProcess[str]:
        command = [
            "cargo",
            "check",
            "--manifest-path",
            str(consumer_manifest),
            "--config",
            flag_for(dep_path),
            "--config",
            "net.offline=true",
        ]
        print(f"+ {' '.join(command)}", flush=True)
        return subprocess.run(
            command,
            cwd=fixture_root,
            check=False,
            text=True,
            capture_output=True,
        )

    old_api = run_check(registry_dep)
    if old_api.returncode == 0:
        raise AssertionError(
            "expected consumer to fail against registry API surface, but cargo "
            f"check succeeded\nstdout:\n{old_api.stdout}\nstderr:\n{old_api.stderr}"
        )
    combined_old = f"{old_api.stdout}\n{old_api.stderr}"
    if "EXTRA_SYMBOL" not in combined_old and "unresolved import" not in combined_old:
        raise AssertionError(
            "registry-surface failure did not look like a missing export\n"
            f"stdout:\n{old_api.stdout}\nstderr:\n{old_api.stderr}"
        )

    new_api = run_check(workspace_dep)
    if new_api.returncode != 0:
        raise AssertionError(
            "expected consumer to succeed against workspace API surface\n"
            f"stdout:\n{new_api.stdout}\nstderr:\n{new_api.stderr}"
        )

    dep = InternalDependency(
        package_name="boundary-dep",
        version="0.1.0",
        package_root=workspace_dep,
    )
    if path_patches_for_dependencies((dep,), version_available=lambda *_: True):
        raise AssertionError("published dependency must not receive a path patch")
    unpublished = path_patches_for_dependencies(
        (dep,), version_available=lambda *_: False
    )
    if unpublished != (PathPatch(package_name="boundary-dep", path=workspace_dep),):
        raise AssertionError(
            f"unpublished dependency must receive a path patch, got {unpublished!r}"
        )

    print("publish-boundary fixture checks passed", flush=True)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--print-cargo-config",
        metavar="PACKAGE",
        help="print cargo --config flags for PACKAGE (one token per line)",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="run the local publish-boundary fixture checks",
    )
    parser.add_argument(
        "--fixture-root",
        type=Path,
        default=DEFAULT_FIXTURE_ROOT,
        help="fixture directory for --self-test",
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = build_parser()
    options = parser.parse_args(argv)

    if options.print_cargo_config:
        return print_cargo_config_flags(options.print_cargo_config)

    if options.self_test:
        run_boundary_fixture(options.fixture_root)
        return 0

    verify_all_packages()
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except RegistryError as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1) from error
    except subprocess.CalledProcessError as error:
        raise SystemExit(error.returncode) from error
