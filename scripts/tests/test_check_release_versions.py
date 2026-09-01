from __future__ import annotations

import unittest

from scripts.check_release_versions import (
    cargo_agrees_with_release_baseline,
    find_workspace_dependency_cycle,
    iter_manifest_dependency_names,
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

    # Covers: renamed `package =` keys and target tables must still produce
    # graph edges, or a cycle through those crates would pass the check.
    def test_resolves_renamed_and_target_specific_dependencies(self) -> None:
        cases = (
            (
                {
                    "dependencies": {
                        "rho-sdk": {"path": "../rho-sdk"},
                        "rho-tools": {
                            "path": "../rho-tools",
                            "package": "rho-agent-tools",
                        },
                    }
                },
                {"rho-sdk", "rho-agent-tools"},
            ),
            (
                {
                    "dependencies": {"anyhow": "1"},
                    "dev-dependencies": {"rho-tui-pty": {"path": "../rho-tui-pty"}},
                    "target": {
                        "cfg(unix)": {
                            "dependencies": {
                                "portable-pty": "0.9",
                                "rho-sdk": {"path": "../rho-sdk"},
                            }
                        }
                    },
                },
                {"anyhow", "rho-tui-pty", "portable-pty", "rho-sdk"},
            ),
        )
        for manifest, expected in cases:
            with self.subTest(manifest=manifest):
                self.assertEqual(iter_manifest_dependency_names(manifest), expected)
