from __future__ import annotations

import unittest
from unittest import mock

from scripts import check_sdk_compatibility


class FeatureModesTests(unittest.TestCase):
    def test_finds_implicit_optional_dependency_features(self) -> None:
        manifest = {
            "features": {"default": []},
            "dependencies": {"transport": {"optional": True}},
        }

        self.assertEqual(
            check_sdk_compatibility.implicit_optional_dependencies(manifest),
            {"transport"},
        )

    def test_explicit_dependency_feature_suppresses_implicit_feature(self) -> None:
        manifest = {
            "features": {
                "default": [],
                "transport": ["dep:transport"],
            },
            "target": {
                "cfg(unix)": {
                    "dependencies": {"transport": {"optional": True}}
                }
            },
        }

        self.assertEqual(
            check_sdk_compatibility.implicit_optional_dependencies(manifest),
            set(),
        )

    def test_only_runs_default_mode_without_named_features(self) -> None:
        self.assertEqual(
            check_sdk_compatibility.feature_modes({"default": []}),
            (("default features (no named features)", ()),),
        )

    def test_runs_all_contract_modes_when_named_features_exist(self) -> None:
        self.assertEqual(
            check_sdk_compatibility.feature_modes(
                {"default": [], "unstable-adapter": []}
            ),
            (
                ("default and no-default features", ()),
                ("all features", ("--all-features",)),
            ),
        )

    @mock.patch.object(check_sdk_compatibility, "run")
    @mock.patch.object(check_sdk_compatibility, "load_toml")
    def test_feature_tests_exclude_benchmarks(
        self, load_toml: mock.Mock, run: mock.Mock
    ) -> None:
        load_toml.return_value = {"features": {"default": []}}

        check_sdk_compatibility.test_features()

        run.assert_called_once_with(
            "cargo",
            "test",
            "-p",
            "rho-sdk",
            "--lib",
            "--tests",
            "--examples",
            "--locked",
        )


if __name__ == "__main__":
    unittest.main()
