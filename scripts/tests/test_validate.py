from __future__ import annotations

import unittest

from scripts import validate


class FastPlanTests(unittest.TestCase):
    def test_checks_one_package_without_all_targets(self) -> None:
        plan = validate.fast_plan("rho-sdk")

        self.assertEqual(
            tuple(step.command for step in plan),
            (
                ("cargo", "fmt", "--all", "--", "--check"),
                ("python3", "scripts/check_architecture.py"),
                ("cargo", "check", "-p", "rho-sdk", "--locked"),
            ),
        )

    def test_adds_selected_integration_test_and_filter(self) -> None:
        plan = validate.fast_plan(
            "rho-coding-agent",
            integration_test="automation_cli",
            test_filter="streams_json_events",
        )

        self.assertEqual(
            plan[-1].command,
            (
                "cargo",
                "test",
                "-p",
                "rho-coding-agent",
                "--test",
                "automation_cli",
                "--locked",
                "streams_json_events",
            ),
        )


class FullPlanTests(unittest.TestCase):
    def test_keeps_broad_clippy_and_normal_workspace_tests(self) -> None:
        commands = {step.label: step.command for step in validate.full_plan()}

        self.assertIn("--all-targets", commands["Run Clippy"])
        self.assertIn("--all-features", commands["Run Clippy"])
        self.assertEqual(
            commands["Run workspace tests"],
            ("cargo", "test", "--workspace", "--locked"),
        )
        self.assertEqual(
            commands["Run documentation tests"],
            (
                "cargo",
                "test",
                "--workspace",
                "--doc",
                "--all-features",
                "--locked",
            ),
        )
        self.assertIn("Test SDK feature modes", commands)
        self.assertIn("Check downstream SDK fixtures", commands)
        self.assertEqual(
            commands["Check docs TUI proof plate"],
            ("bash", "scripts/check_docs_ui_demo.sh", "--check"),
        )


class CargoJobsTests(unittest.TestCase):
    def test_defaults_to_cap(self) -> None:
        self.assertEqual(validate.capped_cargo_jobs(None), "12")

    def test_preserves_lower_setting(self) -> None:
        self.assertEqual(validate.capped_cargo_jobs("6"), "6")

    def test_caps_higher_setting(self) -> None:
        self.assertEqual(validate.capped_cargo_jobs("24"), "12")

    def test_resolves_relative_setting_before_capping(self) -> None:
        self.assertEqual(
            validate.capped_cargo_jobs("-2", cpu_count=8),
            "6",
        )


if __name__ == "__main__":
    unittest.main()
