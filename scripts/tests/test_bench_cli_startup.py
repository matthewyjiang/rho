from __future__ import annotations

import unittest

from scripts import bench_cli_startup


class PeakRssUnitTests(unittest.TestCase):
    # Covers: macOS/BSD allocator receipts reported 1024x too high
    # Owner: OS process probe
    def test_wait4_units_depend_on_platform(self) -> None:
        cases = (
            ("linux", 12_288, 12_288),
            ("darwin", 12_288 * 1024, 12_288),
            ("freebsd14", 12_288 * 1024, 12_288),
        )
        for platform_name, raw, expected_kib in cases:
            with self.subTest(platform_name=platform_name):
                self.assertEqual(
                    bench_cli_startup.peak_rss_kib_from_rusage(
                        raw, platform_name=platform_name
                    ),
                    expected_kib,
                )


if __name__ == "__main__":
    unittest.main()
