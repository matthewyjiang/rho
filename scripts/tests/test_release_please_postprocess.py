from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from scripts import release_please_postprocess as postprocess


class VersionHelpersTests(unittest.TestCase):
    def test_detects_major_bump(self) -> None:
        self.assertTrue(postprocess.is_major_bump("1.24.1", "2.0.0"))
        self.assertFalse(postprocess.is_major_bump("1.24.1", "1.25.0"))
        self.assertFalse(postprocess.is_major_bump("0.15.1", "0.15.2"))

    def test_next_minor(self) -> None:
        self.assertEqual(postprocess.next_minor("1.24.1"), "1.25.0")
        self.assertEqual(postprocess.next_minor("0.15.1"), "0.16.0")


class SyncMinorPinsTests(unittest.TestCase):
    def test_pins_blocked_majors_to_next_minor(self) -> None:
        config = {
            "packages": {
                "crates/rho": {"draft": True},
                "crates/rho-sdk": {"draft": True},
                "crates/rho-tools": {"draft": True},
            }
        }

        updated, required, changed = postprocess.sync_minor_pins(
            config,
            previous_manifest={
                "crates/rho": "1.24.1",
                "crates/rho-sdk": "1.13.1",
                "crates/rho-tools": "0.10.1",
            },
            proposed_manifest={
                "crates/rho": "2.0.0",
                "crates/rho-sdk": "2.0.0",
                "crates/rho-tools": "0.11.0",
            },
            allow_major_releases=False,
        )

        self.assertTrue(changed)
        self.assertEqual(
            required,
            {
                "crates/rho": "1.25.0",
                "crates/rho-sdk": "1.14.0",
            },
        )
        self.assertEqual(
            updated["packages"]["crates/rho"]["release-as"],
            "1.25.0",
        )
        self.assertEqual(
            updated["packages"]["crates/rho-sdk"]["release-as"],
            "1.14.0",
        )
        self.assertNotIn("release-as", updated["packages"]["crates/rho-tools"])

    def test_leaves_existing_matching_pins_untouched(self) -> None:
        config = {
            "packages": {
                "crates/rho": {"release-as": "1.25.0"},
            }
        }

        updated, required, changed = postprocess.sync_minor_pins(
            config,
            previous_manifest={"crates/rho": "1.24.1"},
            proposed_manifest={"crates/rho": "2.0.0"},
            allow_major_releases=False,
        )

        self.assertFalse(changed)
        self.assertEqual(required, {"crates/rho": "1.25.0"})
        self.assertEqual(updated["packages"]["crates/rho"]["release-as"], "1.25.0")

    def test_skips_when_majors_allowed(self) -> None:
        config = {"packages": {"crates/rho": {}}}

        updated, required, changed = postprocess.sync_minor_pins(
            config,
            previous_manifest={"crates/rho": "1.24.1"},
            proposed_manifest={"crates/rho": "2.0.0"},
            allow_major_releases=True,
        )

        self.assertFalse(changed)
        self.assertEqual(required, {})
        self.assertNotIn("release-as", updated["packages"]["crates/rho"])


class ClearSpentPinsTests(unittest.TestCase):
    def test_clears_pins_that_match_manifest(self) -> None:
        config = {
            "packages": {
                "crates/rho": {"release-as": "1.25.0", "draft": True},
                "crates/rho-sdk": {"release-as": "1.14.0"},
                "crates/rho-tools": {"release-as": "0.12.0"},
            }
        }

        updated, cleared = postprocess.clear_spent_pins(
            config,
            manifest={
                "crates/rho": "1.25.0",
                "crates/rho-sdk": "1.14.0",
                "crates/rho-tools": "0.11.0",
            },
        )

        self.assertEqual(cleared, ["crates/rho", "crates/rho-sdk"])
        self.assertNotIn("release-as", updated["packages"]["crates/rho"])
        self.assertNotIn("release-as", updated["packages"]["crates/rho-sdk"])
        self.assertEqual(
            updated["packages"]["crates/rho-tools"]["release-as"],
            "0.12.0",
        )
        self.assertTrue(updated["packages"]["crates/rho"]["draft"])


class StripBreakingNotesTests(unittest.TestCase):
    def test_strips_breaking_section_on_same_major(self) -> None:
        text = """# Changelog

## [1.25.0](https://example.test/compare/v1.24.1...v1.25.0) (2026-07-31)


### ⚠ BREAKING CHANGES

* **hooks:** additive non_exhaustive variant.

### Features

* **hooks:** add typed lifecycle hooks

## [1.24.1](https://example.test/compare/v1.24.0...v1.24.1) (2026-07-30)


### Bug Fixes

* **tui:** show hosted cards
"""
        updated, changed = postprocess.strip_non_major_breaking_notes(text)

        self.assertTrue(changed)
        self.assertNotIn("BREAKING CHANGES", updated)
        self.assertIn("### Features", updated)
        self.assertIn("## [1.24.1]", updated)
        self.assertIn("* **hooks:** add typed lifecycle hooks", updated)

    def test_keeps_breaking_section_on_real_major(self) -> None:
        text = """# Changelog

## [2.0.0](https://example.test/compare/v1.24.1...v2.0.0) (2026-07-31)


### ⚠ BREAKING CHANGES

* **api:** remove old path.

### Features

* **api:** new path

## [1.24.1](https://example.test/compare/v1.24.0...v1.24.1) (2026-07-30)


### Bug Fixes

* patch
"""
        updated, changed = postprocess.strip_non_major_breaking_notes(text)

        self.assertFalse(changed)
        self.assertEqual(updated, text)


class DemoteReleaseTreeTests(unittest.TestCase):
    def test_demotes_release_tree_and_strips_breaking_notes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "crates" / "rho").mkdir(parents=True)
            (root / "crates" / "rho-sdk").mkdir(parents=True)

            config = {
                "packages": {
                    "crates/rho": {
                        "package-name": "rho-coding-agent",
                        "component": "rho-coding-agent",
                        "changelog-path": "CHANGELOG.md",
                        "extra-files": [{"type": "generic", "path": "PKGBUILD"}],
                    },
                    "crates/rho-sdk": {
                        "package-name": "rho-sdk",
                        "component": "rho-sdk",
                        "changelog-path": "CHANGELOG.md",
                    },
                }
            }
            (root / ".release-please-config.json").write_text(
                json.dumps(config, indent=2) + "\n",
                encoding="utf-8",
            )
            (root / ".release-please-manifest.json").write_text(
                json.dumps(
                    {
                        "crates/rho": "2.0.0",
                        "crates/rho-sdk": "2.0.0",
                    },
                    indent=2,
                )
                + "\n",
                encoding="utf-8",
            )
            (root / "crates" / "rho" / "Cargo.toml").write_text(
                '[package]\nname = "rho-coding-agent"\nversion = "2.0.0"\n',
                encoding="utf-8",
            )
            (root / "crates" / "rho-sdk" / "Cargo.toml").write_text(
                '[package]\nname = "rho-sdk"\nversion = "2.0.0"\n',
                encoding="utf-8",
            )
            (root / "crates" / "rho" / "PKGBUILD").write_text(
                "pkgver=2.0.0 # x-release-please-version\n",
                encoding="utf-8",
            )
            (root / "Cargo.lock").write_text(
                '''[[package]]
name = "rho-coding-agent"
version = "2.0.0"

[[package]]
name = "rho-sdk"
version = "2.0.0"
''',
                encoding="utf-8",
            )
            (root / "crates" / "rho" / "CHANGELOG.md").write_text(
                """# Changelog

## [2.0.0](https://example.test/compare/rho-coding-agent-v1.24.1...rho-coding-agent-v2.0.0) (2026-07-31)


### ⚠ BREAKING CHANGES

* **hooks:** additive.

### Features

* **hooks:** feature


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * rho-sdk bumped from 1.13.1 to 2.0.0

## [1.24.1](https://example.test/compare/rho-coding-agent-v1.24.0...rho-coding-agent-v1.24.1) (2026-07-30)


### Bug Fixes

* fix
""",
                encoding="utf-8",
            )
            (root / "crates" / "rho-sdk" / "CHANGELOG.md").write_text(
                """# Changelog

## [2.0.0](https://example.test/compare/rho-sdk-v1.13.1...rho-sdk-v2.0.0) (2026-07-31)


### ⚠ BREAKING CHANGES

* **hooks:** additive.

### Features

* **hooks:** feature

## [1.13.1](https://example.test/compare/rho-sdk-v1.13.0...rho-sdk-v1.13.1) (2026-07-30)


### Bug Fixes

* fix
""",
                encoding="utf-8",
            )

            demotions = postprocess.demote_release_tree(
                root,
                config,
                previous_manifest={
                    "crates/rho": "1.24.1",
                    "crates/rho-sdk": "1.13.1",
                },
                proposed_manifest={
                    "crates/rho": "2.0.0",
                    "crates/rho-sdk": "2.0.0",
                },
                allow_major_releases=False,
            )

            self.assertEqual(
                [(item.path, item.to_version) for item in demotions],
                [("crates/rho", "1.25.0"), ("crates/rho-sdk", "1.14.0")],
            )
            manifest = json.loads(
                (root / ".release-please-manifest.json").read_text(encoding="utf-8")
            )
            self.assertEqual(manifest["crates/rho"], "1.25.0")
            self.assertEqual(manifest["crates/rho-sdk"], "1.14.0")
            self.assertIn(
                'version = "1.25.0"',
                (root / "crates" / "rho" / "Cargo.toml").read_text(encoding="utf-8"),
            )
            self.assertIn(
                "pkgver=1.25.0 # x-release-please-version",
                (root / "crates" / "rho" / "PKGBUILD").read_text(encoding="utf-8"),
            )
            lock = (root / "Cargo.lock").read_text(encoding="utf-8")
            self.assertIn('name = "rho-coding-agent"\nversion = "1.25.0"', lock)
            self.assertIn('name = "rho-sdk"\nversion = "1.14.0"', lock)

            app_changelog = (root / "crates" / "rho" / "CHANGELOG.md").read_text(
                encoding="utf-8"
            )
            self.assertIn("## [1.25.0]", app_changelog)
            self.assertIn("rho-coding-agent-v1.25.0", app_changelog)
            self.assertIn("rho-sdk bumped from 1.13.1 to 1.14.0", app_changelog)
            self.assertNotIn("BREAKING CHANGES", app_changelog)
            self.assertNotIn("2.0.0", app_changelog)

            # Pins are written on main by the workflow, not on the release PR.
            branch_config = json.loads(
                (root / ".release-please-config.json").read_text(encoding="utf-8")
            )
            self.assertNotIn(
                "release-as",
                branch_config["packages"]["crates/rho"],
            )


if __name__ == "__main__":
    unittest.main()
