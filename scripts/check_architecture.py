#!/usr/bin/env python3
"""Enforce lightweight architecture budgets for the rho source tree."""

from __future__ import annotations

import argparse
import tempfile
import unittest
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable

DEFAULT_PRODUCTION_RUST_LINE_BUDGET = 1_000

# Legacy production files that already exceed the default budget. Keep these
# ceilings explicit and lower them as files are split up. New exceptions should
# be avoided in favor of extracting focused modules.
LEGACY_FILE_LINE_BUDGETS = {
    "src/model/openai/codex_ws.rs": 1_036,
    "src/tui.rs": 9_416,
}

# Generated Rust files must be listed by exact repository-relative path with a
# short reason. An explicit list avoids accidentally exempting hand-written
# files that merely mention generated content.
GENERATED_RUST_FILES: dict[str, str] = {}

# Dedicated test files are excluded from production file-size budgets. Inline
# tests remain part of their production file's budget because separating them
# reliably requires Rust-aware parsing.
TEST_FILE_NAMES = {"tests.rs"}
TEST_FILE_SUFFIXES = ("_test.rs", "_tests.rs")

THIN_BINARY_LINE_BUDGETS = {
    "src/main.rs": 50,
}

# Source modules may not depend on the listed crate-root modules. This scanner
# recognizes direct crate paths and first-level branches in `use crate::{...}`
# trees after removing comments and literals.
FORBIDDEN_CRATE_MODULE_DEPENDENCIES = {
    "src/credentials.rs": {"model"},
}


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


def is_dedicated_test_file(relative: str) -> bool:
    path = Path(relative)
    return (
        "tests" in path.parts
        or path.name in TEST_FILE_NAMES
        or path.name.endswith(TEST_FILE_SUFFIXES)
    )


def count_lines(path: Path) -> int:
    return len(path.read_text(encoding="utf-8").splitlines())


def production_rust_files(root: Path) -> list[Path]:
    files = list((root / "src").rglob("*.rs"))
    build_script = root / "build.rs"
    if build_script.is_file():
        files.append(build_script)
    return sorted(files)


def check_file_size_budgets(
    root: Path,
    *,
    legacy_budgets: dict[str, int] | None = None,
    generated_files: dict[str, str] | None = None,
    default_budget: int = DEFAULT_PRODUCTION_RUST_LINE_BUDGET,
) -> SizeCheckResult:
    legacy_budgets = LEGACY_FILE_LINE_BUDGETS if legacy_budgets is None else legacy_budgets
    generated_files = GENERATED_RUST_FILES if generated_files is None else generated_files
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
        if is_dedicated_test_file(relative):
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


def check_dependency_boundaries(root: Path) -> list[str]:
    errors: list[str] = []
    for relative, forbidden_modules in sorted(FORBIDDEN_CRATE_MODULE_DEPENDENCIES.items()):
        path = root / relative
        if not path.is_file():
            errors.append(f"dependency-boundary source does not exist: {relative}")
            continue
        source = path.read_text(encoding="utf-8")
        for module in sorted(forbidden_modules):
            if references_crate_module(source, module):
                errors.append(
                    f"{relative}: must not depend on crate::{module}; keep credentials "
                    "independent from model runtime metadata"
                )
    return errors


def check_thin_binaries(root: Path) -> list[str]:
    errors: list[str] = []
    for relative, budget in sorted(THIN_BINARY_LINE_BUDGETS.items()):
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


def run_checks(root: Path) -> int:
    size_result = check_file_size_budgets(root)
    errors = list(size_result.errors)
    errors.extend(check_dependency_boundaries(root))
    errors.extend(check_thin_binaries(root))

    if errors:
        print_errors(errors)
        print(f"architecture checks failed with {len(errors)} error(s)")
        return 1

    print("architecture checks passed")
    print(f"  production Rust files checked: {size_result.checked_files}")
    print(f"  dedicated test files excluded: {size_result.excluded_test_files}")
    print(f"  generated Rust files excluded: {size_result.excluded_generated_files}")
    print(f"  legacy file-size budgets: {len(LEGACY_FILE_LINE_BUDGETS)}")
    print(f"  dependency boundaries: {len(FORBIDDEN_CRATE_MODULE_DEPENDENCIES)}")
    print(f"  thin binary budgets: {len(THIN_BINARY_LINE_BUDGETS)}")
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
        "--self-test",
        action="store_true",
        help="run deterministic scanner and file-budget self-tests",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.self_test:
        return run_self_tests()
    return run_checks(args.root.resolve())


if __name__ == "__main__":
    raise SystemExit(main())
