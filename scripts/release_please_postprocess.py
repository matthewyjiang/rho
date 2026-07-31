#!/usr/bin/env python3
"""Post-process Release Please output for Rho's release policy.

Release Please treats any `BREAKING CHANGE` commit footer as a major bump.
Rho often marks additive `#[non_exhaustive]` API growth that way by mistake.
Until a real major is intentionally allowed, this script:

1. pins the next release to a minor when the proposed release would major
2. rewrites an open release PR from those majors down to the pinned minors
3. drops generated breaking sections from changelogs that stay on the same major
4. clears spent `release-as` pins after those versions ship

Intentional majors set `allow-major` to true in `.release-policy.json`.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_CONFIG = ROOT / ".release-please-config.json"
DEFAULT_POLICY = ROOT / ".release-policy.json"
DEFAULT_MANIFEST = ROOT / ".release-please-manifest.json"

VERSION_RE = re.compile(r"^(\d+)\.(\d+)\.(\d+)$")
CHANGELOG_HEADER_RE = re.compile(
    r"^## \[(\d+\.\d+\.\d+)\]([^\n]*)\n",
    re.MULTILINE,
)
# Release Please emits a breaking section with a trailing blank line before the
# next ### heading or end of the entry.
BREAKING_SECTION_RE = re.compile(
    r"\n### ⚠ BREAKING CHANGES\n"
    r"\n"
    r"(?:\* .+\n)+"
    r"\n",
)
CARGO_PACKAGE_VERSION_RE = re.compile(
    r'(?m)^version = "(\d+\.\d+\.\d+)"\s*$'
)
PKGBUILD_VERSION_RE = re.compile(
    r"(?m)^pkgver=(\d+\.\d+\.\d+)(\s*# x-release-please-version\s*)$"
)


@dataclass(frozen=True)
class Demotion:
    path: str
    package_name: str
    component: str
    from_version: str
    to_version: str


def load_json(path: Path) -> dict[str, object]:
    return json.loads(path.read_text(encoding="utf-8"))


def dump_json(path: Path, data: dict[str, object]) -> None:
    path.write_text(
        json.dumps(data, indent=2, sort_keys=False) + "\n",
        encoding="utf-8",
    )


def parse_version(version: str) -> tuple[int, int, int]:
    match = VERSION_RE.fullmatch(version)
    if match is None:
        raise ValueError(f"unsupported version: {version!r}")
    return int(match.group(1)), int(match.group(2)), int(match.group(3))


def format_version(version: tuple[int, int, int]) -> str:
    major, minor, patch = version
    return f"{major}.{minor}.{patch}"


def is_major_bump(previous: str, proposed: str) -> bool:
    return parse_version(proposed)[0] > parse_version(previous)[0]


def next_minor(version: str) -> str:
    major, minor, _patch = parse_version(version)
    return format_version((major, minor + 1, 0))


def allow_major(policy: dict[str, object]) -> bool:
    value = policy.get("allow-major", False)
    if not isinstance(value, bool):
        raise ValueError("release policy allow-major must be a boolean")
    return value


def package_entries(config: dict[str, object]) -> dict[str, dict[str, object]]:
    packages = config.get("packages")
    if not isinstance(packages, dict):
        raise ValueError("release-please config packages must be an object")
    entries: dict[str, dict[str, object]] = {}
    for path, entry in packages.items():
        if not isinstance(path, str) or not isinstance(entry, dict):
            raise ValueError("release-please package entries must be objects")
        entries[path] = entry
    return entries


def string_map(data: dict[str, object]) -> dict[str, str]:
    return {str(key): str(value) for key, value in data.items()}


def required_minor_pins(
    *,
    previous_manifest: dict[str, str],
    proposed_manifest: dict[str, str],
    allow_major_releases: bool,
) -> dict[str, str]:
    if allow_major_releases:
        return {}
    pins: dict[str, str] = {}
    for path, proposed in proposed_manifest.items():
        previous = previous_manifest.get(path)
        if previous is None or not is_major_bump(previous, proposed):
            continue
        pins[path] = next_minor(previous)
    return pins


def sync_minor_pins(
    config: dict[str, object],
    *,
    previous_manifest: dict[str, str],
    proposed_manifest: dict[str, str],
    allow_major_releases: bool,
) -> tuple[dict[str, object], dict[str, str], bool]:
    """Add release-as minor pins for blocked major bumps.

    Returns (config, required_pins, changed).
    """
    required = required_minor_pins(
        previous_manifest=previous_manifest,
        proposed_manifest=proposed_manifest,
        allow_major_releases=allow_major_releases,
    )
    if not required:
        return config, {}, False

    packages = package_entries(config)
    changed = False
    for path, pin in required.items():
        entry = packages.get(path)
        if entry is None:
            raise ValueError(f"proposed package {path} missing from config")
        if entry.get("release-as") != pin:
            entry["release-as"] = pin
            changed = True
    return config, required, changed


def clear_spent_pins(
    config: dict[str, object],
    *,
    manifest: dict[str, str],
) -> tuple[dict[str, object], list[str]]:
    """Remove release-as pins that match the shipped manifest version."""
    packages = package_entries(config)
    cleared: list[str] = []
    for path, entry in packages.items():
        pin = entry.get("release-as")
        if not isinstance(pin, str):
            continue
        if manifest.get(path) != pin:
            continue
        del entry["release-as"]
        cleared.append(path)
    return config, cleared


def strip_breaking_section(entry: str) -> str:
    return BREAKING_SECTION_RE.sub("\n", entry, count=1)


def split_changelog_entries(text: str) -> tuple[str, list[tuple[str, str, str]]]:
    """Return preamble and (version, header_suffix, body) entries."""
    matches = list(CHANGELOG_HEADER_RE.finditer(text))
    if not matches:
        return text, []

    preamble = text[: matches[0].start()]
    entries: list[tuple[str, str, str]] = []
    for index, match in enumerate(matches):
        start = match.end()
        end = matches[index + 1].start() if index + 1 < len(matches) else len(text)
        entries.append((match.group(1), match.group(2), text[start:end]))
    return preamble, entries


def strip_non_major_breaking_notes(text: str) -> tuple[str, bool]:
    """Drop breaking sections from top entry when it stays on the same major."""
    preamble, entries = split_changelog_entries(text)
    if len(entries) < 2:
        return text, False

    top_version, top_suffix, top_body = entries[0]
    previous_version = entries[1][0]
    if parse_version(top_version)[0] != parse_version(previous_version)[0]:
        return text, False

    stripped_body = strip_breaking_section(top_body)
    if stripped_body == top_body:
        return text, False

    pieces = [preamble, f"## [{top_version}]{top_suffix}\n", stripped_body]
    for version, suffix, body in entries[1:]:
        pieces.append(f"## [{version}]{suffix}\n")
        pieces.append(body)
    return "".join(pieces), True


def changelog_paths(config: dict[str, object], repo_root: Path) -> list[Path]:
    paths: list[Path] = []
    for package_path, entry in package_entries(config).items():
        changelog_name = entry.get("changelog-path", "CHANGELOG.md")
        if not isinstance(changelog_name, str):
            raise ValueError(f"{package_path} changelog-path must be a string")
        paths.append(repo_root / package_path / changelog_name)
    return paths


def package_name_for(entry: dict[str, object], path: str) -> str:
    name = entry.get("package-name")
    if isinstance(name, str) and name:
        return name
    return Path(path).name


def component_for(entry: dict[str, object], path: str) -> str:
    component = entry.get("component")
    if isinstance(component, str) and component:
        return component
    return package_name_for(entry, path)


def planned_demotions(
    config: dict[str, object],
    *,
    previous_manifest: dict[str, str],
    proposed_manifest: dict[str, str],
    allow_major_releases: bool,
) -> list[Demotion]:
    pins = required_minor_pins(
        previous_manifest=previous_manifest,
        proposed_manifest=proposed_manifest,
        allow_major_releases=allow_major_releases,
    )
    packages = package_entries(config)
    demotions: list[Demotion] = []
    for path, to_version in pins.items():
        entry = packages[path]
        demotions.append(
            Demotion(
                path=path,
                package_name=package_name_for(entry, path),
                component=component_for(entry, path),
                from_version=proposed_manifest[path],
                to_version=to_version,
            )
        )
    return demotions


def replace_cargo_package_version(text: str, new_version: str) -> str:
    updated, count = CARGO_PACKAGE_VERSION_RE.subn(
        f'version = "{new_version}"',
        text,
        count=1,
    )
    if count != 1:
        raise RuntimeError("failed to rewrite package version in Cargo.toml")
    return updated


def replace_pkgbuild_version(text: str, new_version: str) -> str:
    updated, count = PKGBUILD_VERSION_RE.subn(
        rf"pkgver={new_version}\2",
        text,
        count=1,
    )
    if count != 1:
        raise RuntimeError("failed to rewrite pkgver in PKGBUILD")
    return updated


def replace_lock_package_version(
    text: str,
    *,
    package_name: str,
    from_version: str,
    to_version: str,
) -> str:
    pattern = re.compile(
        rf'(?m)^name = "{re.escape(package_name)}"\n'
        rf'version = "{re.escape(from_version)}"$'
    )
    updated, count = pattern.subn(
        f'name = "{package_name}"\nversion = "{to_version}"',
        text,
        count=1,
    )
    if count != 1:
        raise RuntimeError(
            f"failed to rewrite Cargo.lock version for {package_name}"
        )
    return updated


def replace_top_changelog_version(
    text: str,
    *,
    from_version: str,
    to_version: str,
    component: str,
) -> str:
    preamble, entries = split_changelog_entries(text)
    if not entries:
        raise RuntimeError("changelog has no version entries")
    top_version, top_suffix, top_body = entries[0]
    if top_version != from_version:
        raise RuntimeError(
            f"changelog top entry is {top_version}, expected {from_version}"
        )
    suffix = top_suffix.replace(from_version, to_version)
    # Compare URLs use component tags such as rho-sdk-v2.0.0.
    suffix = suffix.replace(
        f"{component}-v{from_version}",
        f"{component}-v{to_version}",
    )
    # Leave the body alone here. Shared major strings can appear in
    # dependency notes for other packages; those are rewritten by name.
    pieces = [preamble, f"## [{to_version}]{suffix}\n", top_body]
    for version, entry_suffix, entry_body in entries[1:]:
        pieces.append(f"## [{version}]{entry_suffix}\n")
        pieces.append(entry_body)
    return "".join(pieces)


def rewrite_dependency_bump_lines(
    text: str,
    *,
    package_name: str,
    from_version: str,
    to_version: str,
) -> str:
    """Rewrite 'package bumped from X to major' lines inside dependency notes."""
    pattern = re.compile(
        rf"(?m)^(\s*\* {re.escape(package_name)} bumped from \d+\.\d+\.\d+ to )"
        rf"{re.escape(from_version)}"
        rf"(\s*)$"
    )
    return pattern.sub(rf"\g<1>{to_version}\2", text)


def demote_release_tree(
    repo_root: Path,
    config: dict[str, object],
    *,
    previous_manifest: dict[str, str],
    proposed_manifest: dict[str, str],
    allow_major_releases: bool,
) -> list[Demotion]:
    """Rewrite a release PR tree so blocked majors become next minors."""
    demotions = planned_demotions(
        config,
        previous_manifest=previous_manifest,
        proposed_manifest=proposed_manifest,
        allow_major_releases=allow_major_releases,
    )
    if not demotions:
        return []

    manifest_path = repo_root / ".release-please-manifest.json"
    manifest = string_map(load_json(manifest_path))
    for demotion in demotions:
        if manifest.get(demotion.path) != demotion.from_version:
            raise RuntimeError(
                f"manifest {demotion.path} is {manifest.get(demotion.path)!r}, "
                f"expected {demotion.from_version!r}"
            )
        manifest[demotion.path] = demotion.to_version
    dump_json(manifest_path, manifest)

    cargo_lock = repo_root / "Cargo.lock"
    cargo_lock_text = cargo_lock.read_text(encoding="utf-8") if cargo_lock.exists() else None

    for demotion in demotions:
        cargo_toml = repo_root / demotion.path / "Cargo.toml"
        cargo_toml.write_text(
            replace_cargo_package_version(
                cargo_toml.read_text(encoding="utf-8"),
                demotion.to_version,
            ),
            encoding="utf-8",
        )

        entry = package_entries(config)[demotion.path]
        for extra in entry.get("extra-files", []) or []:
            if not isinstance(extra, dict):
                continue
            if extra.get("type") != "generic":
                continue
            rel = extra.get("path")
            if not isinstance(rel, str):
                continue
            path = repo_root / demotion.path / rel
            if path.name != "PKGBUILD" or not path.exists():
                continue
            path.write_text(
                replace_pkgbuild_version(
                    path.read_text(encoding="utf-8"),
                    demotion.to_version,
                ),
                encoding="utf-8",
            )

        if cargo_lock_text is not None:
            cargo_lock_text = replace_lock_package_version(
                cargo_lock_text,
                package_name=demotion.package_name,
                from_version=demotion.from_version,
                to_version=demotion.to_version,
            )

    if cargo_lock_text is not None:
        cargo_lock.write_text(cargo_lock_text, encoding="utf-8")

    for changelog in changelog_paths(config, repo_root):
        if not changelog.exists():
            continue
        text = changelog.read_text(encoding="utf-8")
        package_path = str(changelog.parent.relative_to(repo_root))
        for demotion in demotions:
            if demotion.path == package_path:
                text = replace_top_changelog_version(
                    text,
                    from_version=demotion.from_version,
                    to_version=demotion.to_version,
                    component=demotion.component,
                )
            text = rewrite_dependency_bump_lines(
                text,
                package_name=demotion.package_name,
                from_version=demotion.from_version,
                to_version=demotion.to_version,
            )
        # Path package names may differ from publish names (rho-tools vs
        # rho-agent-tools). Also rewrite bare path basenames used in notes.
        for demotion in demotions:
            basename = Path(demotion.path).name
            if basename != demotion.package_name:
                text = rewrite_dependency_bump_lines(
                    text,
                    package_name=basename,
                    from_version=demotion.from_version,
                    to_version=demotion.to_version,
                )
        text, _changed = strip_non_major_breaking_notes(text)
        changelog.write_text(text, encoding="utf-8")

    return demotions


def cmd_sync_minor_pins(args: argparse.Namespace) -> int:
    config_path = Path(args.config)
    policy_path = Path(args.policy)
    previous_manifest = string_map(load_json(Path(args.previous_manifest)))
    proposed_manifest = string_map(load_json(Path(args.proposed_manifest)))
    config = load_json(config_path)
    policy = load_json(policy_path) if policy_path.exists() else {"allow-major": False}

    updated, required, changed = sync_minor_pins(
        config,
        previous_manifest=previous_manifest,
        proposed_manifest=proposed_manifest,
        allow_major_releases=allow_major(policy),
    )
    if not required:
        print("No blocked major bumps found")
        return 0

    for path, pin in sorted(required.items()):
        print(f"required minor pin: {path} -> {pin}")
    if not changed:
        print("release-as pins already satisfied")
        return 0
    dump_json(config_path, updated)
    print(f"updated {config_path}")
    return 0


def cmd_clear_spent_pins(args: argparse.Namespace) -> int:
    config_path = Path(args.config)
    manifest_path = Path(args.manifest)
    config = load_json(config_path)
    manifest = string_map(load_json(manifest_path))
    updated, cleared = clear_spent_pins(config, manifest=manifest)
    if not cleared:
        print("No spent release-as pins to clear")
        return 0
    dump_json(config_path, updated)
    print("cleared spent release-as pins:")
    for path in cleared:
        print(f"  {path}")
    return 0


def cmd_strip_non_major_breaking(args: argparse.Namespace) -> int:
    repo_root = Path(args.repo_root)
    config = load_json(Path(args.config))
    changed_paths: list[Path] = []
    for changelog in changelog_paths(config, repo_root):
        if not changelog.exists():
            continue
        original = changelog.read_text(encoding="utf-8")
        updated, changed = strip_non_major_breaking_notes(original)
        if not changed:
            continue
        changelog.write_text(updated, encoding="utf-8")
        changed_paths.append(changelog)

    if not changed_paths:
        print("No non-major breaking changelog sections to strip")
        return 0
    print("stripped breaking notes from:")
    for path in changed_paths:
        print(f"  {path.relative_to(repo_root)}")
    return 0


def cmd_demote_majors(args: argparse.Namespace) -> int:
    repo_root = Path(args.repo_root)
    config_path = Path(args.config)
    policy_path = Path(args.policy)
    config = load_json(config_path)
    policy = load_json(policy_path) if policy_path.exists() else {"allow-major": False}
    previous_manifest = string_map(load_json(Path(args.previous_manifest)))
    proposed_manifest = string_map(load_json(Path(args.proposed_manifest)))

    demotions = demote_release_tree(
        repo_root,
        config,
        previous_manifest=previous_manifest,
        proposed_manifest=proposed_manifest,
        allow_major_releases=allow_major(policy),
    )
    if not demotions:
        print("No blocked major bumps to demote")
        return 0
    print("demoted blocked majors:")
    for demotion in demotions:
        print(
            f"  {demotion.path}: {demotion.from_version} -> {demotion.to_version}"
        )
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)

    sync = sub.add_parser(
        "sync-minor-pins",
        help=(
            "Write release-as minor pins when the proposed release majors and "
            "allow-major is false"
        ),
    )
    sync.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    sync.add_argument("--policy", type=Path, default=DEFAULT_POLICY)
    sync.add_argument("--previous-manifest", type=Path, required=True)
    sync.add_argument("--proposed-manifest", type=Path, required=True)
    sync.set_defaults(func=cmd_sync_minor_pins)

    clear = sub.add_parser(
        "clear-spent-pins",
        help="Remove release-as pins that match the current manifest versions",
    )
    clear.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    clear.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    clear.set_defaults(func=cmd_clear_spent_pins)

    strip = sub.add_parser(
        "strip-non-major-breaking",
        help=(
            "Remove generated BREAKING CHANGES sections from changelog entries "
            "that stay on the same major"
        ),
    )
    strip.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    strip.add_argument("--repo-root", type=Path, default=ROOT)
    strip.set_defaults(func=cmd_strip_non_major_breaking)

    demote = sub.add_parser(
        "demote-majors",
        help=(
            "Rewrite a release PR tree so blocked major bumps become the next "
            "minor and strip false breaking changelog sections"
        ),
    )
    demote.add_argument("--config", type=Path, default=DEFAULT_CONFIG)
    demote.add_argument("--policy", type=Path, default=DEFAULT_POLICY)
    demote.add_argument("--repo-root", type=Path, default=ROOT)
    demote.add_argument("--previous-manifest", type=Path, required=True)
    demote.add_argument("--proposed-manifest", type=Path, required=True)
    demote.set_defaults(func=cmd_demote_majors)

    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
