#!/usr/bin/env python3

import unittest
from unittest import mock

import collect_context


class ResolveBaseTests(unittest.TestCase):
    def test_explicit_base_resolves_without_fallback(self) -> None:
        with mock.patch.object(
            collect_context, "merge_base", return_value="abc123"
        ) as resolve:
            self.assertEqual(
                collect_context.resolve_base("release/v1"),
                ("release/v1", "abc123"),
            )
            resolve.assert_called_once_with("release/v1")

    def test_invalid_explicit_base_fails(self) -> None:
        with mock.patch.object(collect_context, "merge_base", return_value=None):
            with self.assertRaisesRegex(SystemExit, "missing-release"):
                collect_context.resolve_base("missing-release")

    def test_omitted_base_discovers_default_branch(self) -> None:
        with mock.patch.object(
            collect_context,
            "merge_base",
            side_effect=[None, "def456"],
        ) as resolve:
            self.assertEqual(collect_context.resolve_base(""), ("main", "def456"))
            self.assertEqual(
                resolve.call_args_list,
                [mock.call("origin/main"), mock.call("main")],
            )


class NameStatusTests(unittest.TestCase):
    def test_parser_retains_sources_and_selects_destination_paths(self) -> None:
        output = (
            "M\0plain\tname.py\0"
            "R100\0old.py\0new.py\0"
            "C075\0source.py\0copy.py\0"
        )

        self.assertEqual(
            collect_context.parse_name_status(output),
            [
                collect_context.NameStatus(status="M", path="plain\tname.py"),
                collect_context.NameStatus(
                    status="R100", path="new.py", source="old.py"
                ),
                collect_context.NameStatus(
                    status="C075", path="copy.py", source="source.py"
                ),
            ],
        )


if __name__ == "__main__":
    unittest.main()
