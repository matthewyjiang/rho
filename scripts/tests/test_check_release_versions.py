from __future__ import annotations

import unittest

from scripts.check_release_versions import (
    cargo_agrees_with_release_baseline,
    find_workspace_dependency_cycle,
)


class CargoReleaseBaselineTests(unittest.TestCase):
    # Covers: unpublished Cargo may sit one patch/minor/major ahead of the last
    # tag, but a pre-bumped Release Please baseline must not be accepted as current.
    # Owner: release packaging scripts

    def test_accepts_match_or_one_unpublished_step(self) -> None:
        cases = (
            ("1.1.1", "1.1.1", True),
            ("1.1.2", "1.1.1", True),
            ("1.2.0", "1.1.1", True),
            ("1.2.1", "1.1.1", False),
            ("1.3.0", "1.1.1", False),
            ("2.0.0", "1.1.1", True),
            ("5.0.0", "4.2.0", True),
            ("6.0.0", "4.2.0", False),
            ("1.1.1", "1.2.0", False),
            ("1.1.0", "1.1.1", False),
            ("1.2.0-rc.1", "1.1.1", False),
        )
        for cargo_version, release_version, expected in cases:
            with self.subTest(cargo=cargo_version, released=release_version):
                self.assertEqual(
                    cargo_agrees_with_release_baseline(cargo_version, release_version),
                    expected,
                )


class WorkspaceDependencyGraphTests(unittest.TestCase):
    # Covers: release-please cargo-workspace fails closed on a directed cycle,
    # including through Cargo-legal dev-dependency loops.
    # Owner: release packaging scripts

    def test_reports_directed_cycles_and_accepts_dags(self) -> None:
        cases = (
            ({"app": set(), "sdk": set()}, None),
            (
                {"app": {"sdk"}, "sdk": set(), "tools": {"sdk"}},
                None,
            ),
            (
                {"app": {"harness"}, "harness": {"app"}},
                ("app", "harness", "app"),
            ),
            (
                {"a": {"b"}, "b": {"c"}, "c": {"a"}, "d": {"a"}},
                ("a", "b", "c", "a"),
            ),
        )
        for graph, expected in cases:
            with self.subTest(graph=graph):
                self.assertEqual(find_workspace_dependency_cycle(graph), expected)
