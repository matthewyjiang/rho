#!/usr/bin/env python3
"""Enforce lightweight architecture budgets for a Rust source tree.

The checker itself is repository-agnostic: every repository-specific policy
(size budgets, generated-file exemptions, thin-binary limits, and forbidden
crate dependencies) lives in a JSON config file discovered next to the source
tree. A repository with no config file is checked against the built-in default
line budget only.
"""

from __future__ import annotations

import argparse
import json
import tempfile
import tomllib
import unittest
from dataclasses import dataclass, field
from pathlib import Path
from typing import Iterable

# Applied to every production Rust file that does not have a more specific
# budget. Repositories may override this in their config file.
DEFAULT_PRODUCTION_RUST_LINE_BUDGET = 1_000

# Default filename conventions for dedicated test files, which are excluded
# from production file-size budgets. Inline `#[cfg(test)]` modules remain part
# of their production file's budget because separating them reliably requires
# Rust-aware parsing.
DEFAULT_TEST_FILE_NAMES = ("tests.rs",)
DEFAULT_TEST_FILE_SUFFIXES = ("_test.rs", "_tests.rs")

# Config file discovered relative to the checked source tree, unless overridden
# on the command line.
DEFAULT_CONFIG_RELATIVE_PATH = "scripts/architecture.json"


@dataclass(frozen=True)
class ForbiddenDependency:
    """A source file forbidden from importing certain crate-root modules."""

    path: str
    modules: tuple[str, ...]
    reason: str


@dataclass(frozen=True)
class ForbiddenPackageDependency:
    """A Cargo manifest forbidden from depending on specified packages."""

    manifest: str
    packages: tuple[str, ...]
    reason: str


@dataclass(frozen=True)
class ArchitectureConfig:
    """Repository-specific architecture policy loaded from a config file."""

    default_production_line_budget: int = DEFAULT_PRODUCTION_RUST_LINE_BUDGET
    # Legacy production files that already exceed the default budget. Keep these
    # ceilings explicit and lower them as files are split up. New exceptions
    # should be avoided in favor of extracting focused modules.
    legacy_file_budgets: dict[str, int] = field(default_factory=dict)
    # Generated Rust files listed by exact repository-relative path with a short
    # reason. An explicit list avoids accidentally exempting hand-written files
    # that merely mention generated content.
    generated_files: dict[str, str] = field(default_factory=dict)
    # Thin entrypoints (binaries) that should stay small and delegate to the
    # library crate.
    thin_binary_budgets: dict[str, int] = field(default_factory=dict)
    # Source files that may not depend on the listed crate-root modules.
    forbidden_dependencies: tuple[ForbiddenDependency, ...] = ()
    # Cargo package dependencies forbidden across a crate boundary.
    forbidden_package_dependencies: tuple[ForbiddenPackageDependency, ...] = ()
    test_file_names: tuple[str, ...] = DEFAULT_TEST_FILE_NAMES
    test_file_suffixes: tuple[str, ...] = DEFAULT_TEST_FILE_SUFFIXES


class ConfigError(ValueError):
    """Raised when a config file is malformed."""


@dataclass(frozen=True)
class SizeCheckResult:
    checked_files: int
    excluded_test_files: int
    excluded_generated_files: int
    errors: tuple[str, ...]


def repository_root() -> Path:
    return Path(__file__).resolve().parent.parent


def relative_path(path: Path, root: Path) -> str:
    return path.relative_to(root).as_posix()


def _require(condition: bool, message: str) -> None:
    if not condition:
        raise ConfigError(message)


def _string_int_map(raw: object, name: str) -> dict[str, int]:
    _require(isinstance(raw, dict), f"{name} must be an object")
    result: dict[str, int] = {}
    for key, value in raw.items():  # type: ignore[union-attr]
        _require(isinstance(key, str), f"{name} keys must be strings")
        _require(
            isinstance(value, int) and not isinstance(value, bool),
            f"{name}[{key!r}] must be an integer",
        )
        result[key] = value
    return result


def _string_string_map(raw: object, name: str) -> dict[str, str]:
    _require(isinstance(raw, dict), f"{name} must be an object")
    result: dict[str, str] = {}
    for key, value in raw.items():  # type: ignore[union-attr]
        _require(isinstance(key, str), f"{name} keys must be strings")
        _require(isinstance(value, str), f"{name}[{key!r}] must be a string")
        result[key] = value
    return result


def _string_tuple(raw: object, name: str, default: tuple[str, ...]) -> tuple[str, ...]:
    if raw is None:
        return default
    _require(isinstance(raw, list), f"{name} must be an array")
    for value in raw:  # type: ignore[union-attr]
        _require(isinstance(value, str), f"{name} entries must be strings")
    return tuple(raw)  # type: ignore[arg-type]


def _forbidden_dependencies(raw: object) -> tuple[ForbiddenDependency, ...]:
    if raw is None:
        return ()
    _require(isinstance(raw, list), "forbidden_dependencies must be an array")
    entries: list[ForbiddenDependency] = []
    for index, item in enumerate(raw):  # type: ignore[union-attr]
        label = f"forbidden_dependencies[{index}]"
        _require(isinstance(item, dict), f"{label} must be an object")
        path = item.get("path")
        modules = item.get("modules")
        reason = item.get("reason", "")
        _require(isinstance(path, str) and path, f"{label}.path must be a non-empty string")
        _require(isinstance(modules, list) and modules, f"{label}.modules must be a non-empty array")
        for module in modules:
            _require(isinstance(module, str) and module, f"{label}.modules entries must be non-empty strings")
        _require(isinstance(reason, str), f"{label}.reason must be a string")
        entries.append(ForbiddenDependency(path=path, modules=tuple(modules), reason=reason))
    return tuple(entries)


def _forbidden_package_dependencies(raw: object) -> tuple[ForbiddenPackageDependency, ...]:
    if raw is None:
        return ()
    _require(isinstance(raw, list), "forbidden_package_dependencies must be an array")
    entries: list[ForbiddenPackageDependency] = []
    for index, item in enumerate(raw):  # type: ignore[union-attr]
        label = f"forbidden_package_dependencies[{index}]"
        _require(isinstance(item, dict), f"{label} must be an object")
        manifest = item.get("manifest")
        packages = item.get("packages")
        reason = item.get("reason", "")
        _require(
            isinstance(manifest, str) and manifest,
            f"{label}.manifest must be a non-empty string",
        )
        _require(
            isinstance(packages, list) and packages,
            f"{label}.packages must be a non-empty array",
        )
        for package in packages:
            _require(
                isinstance(package, str) and package,
                f"{label}.packages entries must be non-empty strings",
            )
        _require(isinstance(reason, str), f"{label}.reason must be a string")
        entries.append(
            ForbiddenPackageDependency(
                manifest=manifest,
                packages=tuple(packages),
                reason=reason,
            )
        )
    return tuple(entries)


def parse_config(data: object) -> ArchitectureConfig:
    _require(isinstance(data, dict), "config root must be an object")
    default_budget = data.get("default_production_line_budget", DEFAULT_PRODUCTION_RUST_LINE_BUDGET)
    _require(
        isinstance(default_budget, int) and not isinstance(default_budget, bool),
        "default_production_line_budget must be an integer",
    )
    return ArchitectureConfig(
        default_production_line_budget=default_budget,
        legacy_file_budgets=_string_int_map(data.get("legacy_file_budgets", {}), "legacy_file_budgets"),
        generated_files=_string_string_map(data.get("generated_files", {}), "generated_files"),
        thin_binary_budgets=_string_int_map(data.get("thin_binary_budgets", {}), "thin_binary_budgets"),
        forbidden_dependencies=_forbidden_dependencies(data.get("forbidden_dependencies")),
        forbidden_package_dependencies=_forbidden_package_dependencies(
            data.get("forbidden_package_dependencies")
        ),
        test_file_names=_string_tuple(data.get("test_file_names"), "test_file_names", DEFAULT_TEST_FILE_NAMES),
        test_file_suffixes=_string_tuple(
            data.get("test_file_suffixes"), "test_file_suffixes", DEFAULT_TEST_FILE_SUFFIXES
        ),
    )


def load_config(path: Path) -> ArchitectureConfig:
    """Load policy from ``path``; return built-in defaults if it does not exist."""
    if not path.is_file():
        return ArchitectureConfig()
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as error:
        raise ConfigError(f"{path}: invalid JSON: {error}") from error
    return parse_config(data)


def is_dedicated_test_file(
    relative: str,
    *,
    names: Iterable[str] = DEFAULT_TEST_FILE_NAMES,
    suffixes: Iterable[str] = DEFAULT_TEST_FILE_SUFFIXES,
) -> bool:
    path = Path(relative)
    return (
        "tests" in path.parts
        or path.name in set(names)
        or path.name.endswith(tuple(suffixes))
    )


def count_lines(path: Path) -> int:
    return len(path.read_text(encoding="utf-8").splitlines())


def production_rust_files(root: Path) -> list[Path]:
    source_roots = [root / "src"]
    crates_directory = root / "crates"
    if crates_directory.is_dir():
        source_roots.extend(path / "src" for path in crates_directory.iterdir() if path.is_dir())

    files = [path for source_root in source_roots for path in source_root.rglob("*.rs")]
    build_scripts = [root / "build.rs"]
    if crates_directory.is_dir():
        build_scripts.extend(path / "build.rs" for path in crates_directory.iterdir() if path.is_dir())
    files.extend(path for path in build_scripts if path.is_file())
    return sorted(files)


def check_file_size_budgets(
    root: Path,
    *,
    legacy_budgets: dict[str, int],
    generated_files: dict[str, str],
    default_budget: int = DEFAULT_PRODUCTION_RUST_LINE_BUDGET,
    test_file_names: Iterable[str] = DEFAULT_TEST_FILE_NAMES,
    test_file_suffixes: Iterable[str] = DEFAULT_TEST_FILE_SUFFIXES,
) -> SizeCheckResult:
    discovered = production_rust_files(root)
    discovered_paths = {relative_path(path, root) for path in discovered}
    errors: list[str] = []

    for relative in sorted(legacy_budgets):
        if relative not in discovered_paths:
            errors.append(f"legacy size-budget entry does not exist: {relative}")
        elif legacy_budgets[relative] <= default_budget:
            errors.append(
                f"legacy size-budget entry is no longer needed: {relative} "
                f"({legacy_budgets[relative]} <= {default_budget})"
            )

    for relative in sorted(generated_files):
        if relative not in discovered_paths:
            errors.append(f"generated-file exclusion does not exist: {relative}")
        elif not generated_files[relative].strip():
            errors.append(f"generated-file exclusion needs a reason: {relative}")

    checked_files = 0
    excluded_test_files = 0
    excluded_generated_files = 0
    for path in discovered:
        relative = relative_path(path, root)
        if is_dedicated_test_file(relative, names=test_file_names, suffixes=test_file_suffixes):
            excluded_test_files += 1
            continue
        if relative in generated_files:
            excluded_generated_files += 1
            continue

        checked_files += 1
        lines = count_lines(path)
        budget = legacy_budgets.get(relative, default_budget)
        if lines > budget:
            policy = "legacy budget" if relative in legacy_budgets else "production-file budget"
            errors.append(f"{relative}: {lines} lines exceeds {policy} of {budget}")

    return SizeCheckResult(
        checked_files=checked_files,
        excluded_test_files=excluded_test_files,
        excluded_generated_files=excluded_generated_files,
        errors=tuple(errors),
    )


def rust_tokens(source: str) -> list[str]:
    """Return identifiers and structural punctuation, excluding comments/literals."""
    tokens: list[str] = []
    index = 0
    length = len(source)

    while index < length:
        char = source[index]
        next_char = source[index + 1] if index + 1 < length else ""

        if char.isspace():
            index += 1
            continue

        if char == "/" and next_char == "/":
            newline = source.find("\n", index + 2)
            index = length if newline == -1 else newline + 1
            continue

        if char == "/" and next_char == "*":
            depth = 1
            index += 2
            while index < length and depth:
                pair = source[index : index + 2]
                if pair == "/*":
                    depth += 1
                    index += 2
                elif pair == "*/":
                    depth -= 1
                    index += 2
                else:
                    index += 1
            continue

        raw_prefix_length = 0
        if char == "r":
            raw_prefix_length = 1
        elif char in {"b", "c"} and next_char == "r":
            raw_prefix_length = 2
        if raw_prefix_length:
            marker = index + raw_prefix_length
            hashes = 0
            while marker + hashes < length and source[marker + hashes] == "#":
                hashes += 1
            quote = marker + hashes
            if quote < length and source[quote] == '"':
                terminator = '"' + ("#" * hashes)
                end = source.find(terminator, quote + 1)
                index = length if end == -1 else end + len(terminator)
                continue

        string_prefix_length = 1 if char in {"b", "c"} and next_char == '"' else 0
        if char == '"' or string_prefix_length:
            index += string_prefix_length + 1
            while index < length:
                if source[index] == "\\":
                    index += 2
                elif source[index] == '"':
                    index += 1
                    break
                else:
                    index += 1
            continue

        if char == "'":
            # Skip character literals while leaving Rust lifetimes available as
            # ordinary identifier tokens. A closing quote within one escaped or
            # one unescaped character distinguishes the literal forms we need.
            if index + 2 < length and source[index + 2] == "'":
                index += 3
                continue
            if index + 3 < length and next_char == "\\" and source[index + 3] == "'":
                index += 4
                continue
            index += 1
            continue

        if char.isalpha() or char == "_":
            end = index + 1
            while end < length and (source[end].isalnum() or source[end] == "_"):
                end += 1
            tokens.append(source[index:end])
            index = end
            continue

        if char == ":" and next_char == ":":
            tokens.append("::")
            index += 2
            continue

        if char in "{};,":
            tokens.append(char)
        index += 1

    return tokens


def references_crate_module(source: str, module: str) -> bool:
    tokens = rust_tokens(source)

    for index in range(len(tokens) - 2):
        if tokens[index : index + 3] == ["crate", "::", module]:
            return True

    for index in range(len(tokens) - 3):
        if tokens[index : index + 4] != ["use", "crate", "::", "{"]:
            continue

        depth = 1
        branch_start = True
        cursor = index + 4
        while cursor < len(tokens) and depth:
            token = tokens[cursor]
            if token == "{":
                depth += 1
            elif token == "}":
                depth -= 1
            elif depth == 1 and token == ",":
                branch_start = True
            elif depth == 1 and branch_start:
                if token == module:
                    return True
                branch_start = False
            cursor += 1

    return False


def check_dependency_boundaries(
    root: Path, forbidden_dependencies: Iterable[ForbiddenDependency]
) -> list[str]:
    errors: list[str] = []
    for dependency in sorted(forbidden_dependencies, key=lambda entry: entry.path):
        path = root / dependency.path
        if path.is_file():
            sources = [path]
        elif path.is_dir():
            sources = sorted(path.rglob("*.rs"))
        else:
            errors.append(f"dependency-boundary source does not exist: {dependency.path}")
            continue
        for source_path in sources:
            source = source_path.read_text(encoding="utf-8")
            relative = relative_path(source_path, root)
            for module in sorted(dependency.modules):
                if references_crate_module(source, module):
                    message = f"{relative}: must not depend on crate::{module}"
                    if dependency.reason:
                        message += f"; {dependency.reason}"
                    errors.append(message)
    return errors


def manifest_dependency_packages(manifest: dict[str, object]) -> set[str]:
    """Return package names from top-level and target-specific dependency tables."""
    packages: set[str] = set()

    def add_dependencies(table: object) -> None:
        if not isinstance(table, dict):
            return
        for dependency_name, specification in table.items():
            if not isinstance(dependency_name, str):
                continue
            if isinstance(specification, dict):
                package = specification.get("package", dependency_name)
                if isinstance(package, str):
                    packages.add(package)
            else:
                packages.add(dependency_name)

    for table_name in ("dependencies", "dev-dependencies", "build-dependencies"):
        add_dependencies(manifest.get(table_name))
    targets = manifest.get("target")
    if isinstance(targets, dict):
        for target in targets.values():
            if not isinstance(target, dict):
                continue
            for table_name in ("dependencies", "dev-dependencies", "build-dependencies"):
                add_dependencies(target.get(table_name))
    return packages


def check_package_dependency_boundaries(
    root: Path,
    forbidden_dependencies: Iterable[ForbiddenPackageDependency],
) -> list[str]:
    errors: list[str] = []
    for boundary in sorted(forbidden_dependencies, key=lambda entry: entry.manifest):
        path = root / boundary.manifest
        if not path.is_file():
            errors.append(f"dependency-boundary manifest does not exist: {boundary.manifest}")
            continue
        with path.open("rb") as file:
            manifest = tomllib.load(file)
        dependencies = manifest_dependency_packages(manifest)
        for package in sorted(boundary.packages):
            if package in dependencies:
                message = f"{boundary.manifest}: must not depend on package {package}"
                if boundary.reason:
                    message += f"; {boundary.reason}"
                errors.append(message)
    return errors


def check_thin_binaries(root: Path, thin_binary_budgets: dict[str, int]) -> list[str]:
    errors: list[str] = []
    for relative, budget in sorted(thin_binary_budgets.items()):
        path = root / relative
        if not path.is_file():
            errors.append(f"thin-binary entry does not exist: {relative}")
            continue
        lines = count_lines(path)
        if lines > budget:
            errors.append(f"{relative}: {lines} lines exceeds thin-binary budget of {budget}")
    return errors


def print_errors(errors: Iterable[str]) -> None:
    for error in errors:
        print(f"ERROR: {error}")


def run_checks(root: Path, config: ArchitectureConfig) -> int:
    size_result = check_file_size_budgets(
        root,
        legacy_budgets=config.legacy_file_budgets,
        generated_files=config.generated_files,
        default_budget=config.default_production_line_budget,
        test_file_names=config.test_file_names,
        test_file_suffixes=config.test_file_suffixes,
    )
    errors = list(size_result.errors)
    errors.extend(check_dependency_boundaries(root, config.forbidden_dependencies))
    errors.extend(
        check_package_dependency_boundaries(root, config.forbidden_package_dependencies)
    )
    errors.extend(check_thin_binaries(root, config.thin_binary_budgets))

    if errors:
        print_errors(errors)
        print(f"architecture checks failed with {len(errors)} error(s)")
        return 1

    print("architecture checks passed")
    print(f"  production Rust files checked: {size_result.checked_files}")
    print(f"  dedicated test files excluded: {size_result.excluded_test_files}")
    print(f"  generated Rust files excluded: {size_result.excluded_generated_files}")
    print(f"  legacy file-size budgets: {len(config.legacy_file_budgets)}")
    print(f"  source dependency boundaries: {len(config.forbidden_dependencies)}")
    print(f"  package dependency boundaries: {len(config.forbidden_package_dependencies)}")
    print(f"  thin binary budgets: {len(config.thin_binary_budgets)}")
    return 0


class ArchitectureCheckTests(unittest.TestCase):
    def test_dependency_scanner_handles_direct_and_grouped_crate_imports(self) -> None:
        self.assertTrue(references_crate_module("use crate::model::Catalog;", "model"))
        self.assertTrue(
            references_crate_module("use crate::{provider, model::{Catalog, Model}};", "model")
        )
        self.assertTrue(
            references_crate_module("let value = crate::model::Model::default();", "model")
        )

    def test_dependency_scanner_ignores_comments_literals_and_nested_items(self) -> None:
        source = r'''
            // use crate::model::Catalog;
            const EXAMPLE: &str = "crate::model::Catalog";
            const RAW: &str = r#"use crate::{model::Catalog};"#;
            use crate::{provider::{self, model}};
        '''
        self.assertFalse(references_crate_module(source, "model"))

    def test_size_checks_exclude_tests_and_enforce_default_and_legacy_budgets(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "src").mkdir()
            (root / "src/lib.rs").write_text("line\n" * 4, encoding="utf-8")
            (root / "src/large.rs").write_text("line\n" * 6, encoding="utf-8")
            (root / "src/large_tests.rs").write_text("line\n" * 20, encoding="utf-8")

            result = check_file_size_budgets(
                root,
                legacy_budgets={"src/large.rs": 5},
                generated_files={},
                default_budget=4,
            )

            self.assertEqual(result.checked_files, 2)
            self.assertEqual(result.excluded_test_files, 1)
            self.assertEqual(
                result.errors,
                ("src/large.rs: 6 lines exceeds legacy budget of 5",),
            )

    def test_load_config_returns_defaults_when_file_is_absent(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            config = load_config(Path(directory) / "missing.json")
            self.assertEqual(config, ArchitectureConfig())

    def test_parse_config_reads_policy_and_forbidden_dependencies(self) -> None:
        config = parse_config(
            {
                "default_production_line_budget": 800,
                "legacy_file_budgets": {"src/big.rs": 900},
                "generated_files": {"src/gen.rs": "protobuf output"},
                "thin_binary_budgets": {"src/main.rs": 40},
                "forbidden_dependencies": [
                    {"path": "src/a.rs", "modules": ["model", "web"], "reason": "keep it clean"}
                ],
                "forbidden_package_dependencies": [
                    {
                        "manifest": "crates/sdk/Cargo.toml",
                        "packages": ["application"],
                        "reason": "one-way dependency",
                    }
                ],
            }
        )
        self.assertEqual(config.default_production_line_budget, 800)
        self.assertEqual(config.legacy_file_budgets, {"src/big.rs": 900})
        self.assertEqual(config.thin_binary_budgets, {"src/main.rs": 40})
        self.assertEqual(
            config.forbidden_dependencies,
            (ForbiddenDependency(path="src/a.rs", modules=("model", "web"), reason="keep it clean"),),
        )

        self.assertEqual(
            config.forbidden_package_dependencies,
            (
                ForbiddenPackageDependency(
                    manifest="crates/sdk/Cargo.toml",
                    packages=("application",),
                    reason="one-way dependency",
                ),
            ),
        )

    def test_parse_config_rejects_malformed_entries(self) -> None:
        with self.assertRaises(ConfigError):
            parse_config({"legacy_file_budgets": {"src/a.rs": "nope"}})
        with self.assertRaises(ConfigError):
            parse_config({"forbidden_dependencies": [{"modules": ["model"]}]})
        with self.assertRaises(ConfigError):
            parse_config({"forbidden_package_dependencies": [{"packages": ["app"]}]})

    def test_dependency_boundary_message_includes_optional_reason(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "src").mkdir()
            (root / "src/a.rs").write_text("use crate::model::Thing;\n", encoding="utf-8")

            with_reason = check_dependency_boundaries(
                root, [ForbiddenDependency("src/a.rs", ("model",), "because layering")]
            )
            without_reason = check_dependency_boundaries(
                root, [ForbiddenDependency("src/a.rs", ("model",), "")]
            )

            self.assertEqual(
                with_reason, ["src/a.rs: must not depend on crate::model; because layering"]
            )
            self.assertEqual(without_reason, ["src/a.rs: must not depend on crate::model"])

    def test_dependency_boundary_scans_source_directory(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "crates/sdk/src"
            source.mkdir(parents=True)
            (source / "lib.rs").write_text("mod safe;\n", encoding="utf-8")
            (source / "bad.rs").write_text("use crate::tui::Event;\n", encoding="utf-8")

            errors = check_dependency_boundaries(
                root,
                [ForbiddenDependency("crates/sdk/src", ("tui",), "headless")],
            )

            self.assertEqual(
                errors,
                ["crates/sdk/src/bad.rs: must not depend on crate::tui; headless"],
            )

    def test_package_dependency_boundary_handles_aliases_and_targets(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            crate = root / "crates/sdk"
            crate.mkdir(parents=True)
            (crate / "Cargo.toml").write_text(
                """
                [package]
                name = "sdk"
                version = "0.1.0"

                [dependencies]
                app = { package = "application", version = "1" }

                [target.'cfg(windows)'.build-dependencies]
                terminal = "1"
                """,
                encoding="utf-8",
            )

            errors = check_package_dependency_boundaries(
                root,
                [
                    ForbiddenPackageDependency(
                        "crates/sdk/Cargo.toml",
                        ("application", "terminal"),
                        "one-way",
                    )
                ],
            )

            self.assertEqual(
                errors,
                [
                    "crates/sdk/Cargo.toml: must not depend on package application; one-way",
                    "crates/sdk/Cargo.toml: must not depend on package terminal; one-way",
                ],
            )

    def test_production_files_include_workspace_crates(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "src").mkdir()
            sdk_source = root / "crates/sdk/src"
            sdk_source.mkdir(parents=True)
            (root / "src/lib.rs").write_text("", encoding="utf-8")
            (sdk_source / "lib.rs").write_text("", encoding="utf-8")

            self.assertEqual(
                [relative_path(path, root) for path in production_rust_files(root)],
                ["crates/sdk/src/lib.rs", "src/lib.rs"],
            )


def run_self_tests() -> int:
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(ArchitectureCheckTests)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    return 0 if result.wasSuccessful() else 1


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        type=Path,
        default=repository_root(),
        help="repository root to check (defaults to the script's repository)",
    )
    parser.add_argument(
        "--config",
        type=Path,
        default=None,
        help=(
            "path to the architecture policy config "
            f"(defaults to <root>/{DEFAULT_CONFIG_RELATIVE_PATH}; "
            "built-in defaults are used when it is absent)"
        ),
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="run deterministic scanner, config, and file-budget self-tests",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.self_test:
        return run_self_tests()
    root = args.root.resolve()
    config_path = args.config if args.config is not None else root / DEFAULT_CONFIG_RELATIVE_PATH
    try:
        config = load_config(config_path)
    except ConfigError as error:
        print(f"ERROR: {error}")
        return 1
    return run_checks(root, config)


if __name__ == "__main__":
    raise SystemExit(main())
