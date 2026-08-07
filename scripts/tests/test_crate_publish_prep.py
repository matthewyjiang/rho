from __future__ import annotations

import io
import unittest
from pathlib import Path

from scripts import crate_publish_prep as prep



ROOT = Path(__file__).resolve().parents[2]


def _dep(
    name: str, version: str, path: str = "crates/example"
) -> prep.InternalDependency:
    return prep.InternalDependency(
        package_name=name,
        version=version,
        package_root=ROOT / path,
    )


class PathPatchPolicyTests(unittest.TestCase):
    # Covers: published internal dep versions must verify against crates.io
    # Owner: release packaging scripts

    def test_omits_patch_when_exact_version_is_on_crates_io(self) -> None:
        deps = (_dep("rho-sdk", "1.17.1", "crates/rho-sdk"),)
        patches = prep.path_patches_for_dependencies(
            deps, version_available=lambda name, version: True
        )
        self.assertEqual(patches, ())

    def test_keeps_patch_when_version_is_unpublished(self) -> None:
        deps = (
            _dep("rho-sdk", "1.18.0", "crates/rho-sdk"),
            _dep("rho-providers", "0.18.0", "crates/rho-providers"),
        )
        available = {("rho-sdk", "1.18.0"): False, ("rho-providers", "0.18.0"): True}
        patches = prep.path_patches_for_dependencies(
            deps,
            version_available=lambda name, version: available[(name, version)],
        )
        self.assertEqual(
            patches,
            (
                prep.PathPatch(
                    package_name="rho-sdk",
                    path=ROOT / "crates" / "rho-sdk",
                ),
            ),
        )

    def test_caches_registry_lookups_across_packages(self) -> None:
        calls: list[tuple[str, str]] = []

        def probe(name: str, version: str) -> bool:
            calls.append((name, version))
            return False

        cache: dict[tuple[str, str], bool] = {}
        deps = (_dep("rho-sdk", "1.18.0", "crates/rho-sdk"),)
        first = prep.path_patches_for_dependencies(
            deps, version_available=probe, cache=cache
        )
        second = prep.path_patches_for_dependencies(
            deps, version_available=probe, cache=cache
        )
        self.assertEqual(first, second)
        self.assertEqual(calls, [("rho-sdk", "1.18.0")])


class CargoConfigFlagTests(unittest.TestCase):
    def test_uses_package_name_and_relative_path(self) -> None:
        patch = prep.PathPatch(
            package_name="rho-agent-tools",
            path=ROOT / "crates" / "rho-tools",
        )
        self.assertEqual(
            patch.cargo_config_flag(relative_to=ROOT),
            'patch.crates-io.rho-agent-tools.path="crates/rho-tools"',
        )
        self.assertEqual(
            prep.cargo_config_flags((patch,), relative_to=ROOT),
            (
                "--config",
                'patch.crates-io.rho-agent-tools.path="crates/rho-tools"',
            ),
        )


class CratesIoProbeTests(unittest.TestCase):
    # Covers: missing versions and hard registry failures
    # Owner: release packaging scripts

    def test_missing_version_is_unpublished(self) -> None:
        def opener(request, timeout=0):  # noqa: ANN001, ANN202
            raise prep.urllib.error.HTTPError(
                url=request.full_url,
                code=404,
                msg="not found",
                hdrs=None,
                fp=io.BytesIO(),
            )

        self.assertFalse(
            prep.crates_io_version_available(
                "rho-sdk", "9.9.9", opener=opener
            )
        )

    def test_yanked_version_still_counts_as_published(self) -> None:
        payload = b'{"version":{"yanked":true,"num":"1.0.0"}}'

        class Response(io.BytesIO):
            status = 200

            def __init__(self) -> None:
                super().__init__(payload)

            def getcode(self) -> int:
                return 200

            def __enter__(self) -> Response:
                return self

            def __exit__(self, *args: object) -> bool:
                return False

        def opener(request, timeout=0):  # noqa: ANN001, ANN202
            return Response()

        self.assertTrue(
            prep.crates_io_version_available(
                "rho-sdk", "1.0.0", opener=opener
            )
        )

    def test_http_500_fails_closed(self) -> None:
        def opener(request, timeout=0):  # noqa: ANN001, ANN202
            raise prep.urllib.error.HTTPError(
                url=request.full_url,
                code=500,
                msg="error",
                hdrs=None,
                fp=io.BytesIO(),
            )

        with self.assertRaises(prep.RegistryError):
            prep.crates_io_version_available("rho-sdk", "1.0.0", opener=opener)

    def test_transport_error_fails_closed(self) -> None:
        def opener(request, timeout=0):  # noqa: ANN001, ANN202
            raise prep.urllib.error.URLError("offline")

        with self.assertRaises(prep.RegistryError):
            prep.crates_io_version_available("rho-sdk", "1.0.0", opener=opener)


class WorkspaceGraphTests(unittest.TestCase):
    def test_resolves_renamed_tools_dependency_to_package_name(self) -> None:
        metadata = prep.load_metadata(root=ROOT)
        deps = prep.internal_dependencies_for("rho-coding-agent", metadata)
        names = {dep.package_name for dep in deps}
        self.assertIn("rho-agent-tools", names)
        self.assertIn("rho-providers", names)
        self.assertIn("rho-sdk", names)
        tools = next(dep for dep in deps if dep.package_name == "rho-agent-tools")
        self.assertTrue(str(tools.package_root).endswith("crates/rho-tools"))

    def test_providers_only_depends_on_sdk(self) -> None:
        metadata = prep.load_metadata(root=ROOT)
        deps = prep.internal_dependencies_for("rho-providers", metadata)
        self.assertEqual([dep.package_name for dep in deps], ["rho-sdk"])


class VerifyCommandTests(unittest.TestCase):
    def test_verify_package_uses_selective_flags(self) -> None:
        commands: list[tuple[str, ...]] = []

        def runner(*arguments: str, cwd=None) -> None:  # noqa: ANN001
            commands.append(arguments)

        metadata = {
            "packages": [
                {
                    "name": "rho-sdk",
                    "version": "1.17.1",
                    "manifest_path": str(ROOT / "crates" / "rho-sdk" / "Cargo.toml"),
                    "dependencies": [],
                },
                {
                    "name": "rho-agent-tools",
                    "version": "0.12.5",
                    "manifest_path": str(ROOT / "crates" / "rho-tools" / "Cargo.toml"),
                    "dependencies": [],
                },
                {
                    "name": "rho-providers",
                    "version": "0.18.0",
                    "manifest_path": str(
                        ROOT / "crates" / "rho-providers" / "Cargo.toml"
                    ),
                    "dependencies": [
                        {
                            "name": "rho-sdk",
                            "path": str(ROOT / "crates" / "rho-sdk"),
                            "kind": None,
                        }
                    ],
                },
                {
                    "name": "rho-coding-agent",
                    "version": "1.32.0",
                    "manifest_path": str(ROOT / "crates" / "rho" / "Cargo.toml"),
                    "dependencies": [],
                },
            ]
        }

        prep.verify_package(
            "rho-providers",
            metadata=metadata,
            root=ROOT,
            version_available=lambda name, version: True,
            runner=runner,
        )
        self.assertEqual(
            commands,
            [
                (
                    "cargo",
                    "publish",
                    "--dry-run",
                    "--locked",
                    "-p",
                    "rho-providers",
                )
            ],
        )

        commands.clear()
        prep.verify_package(
            "rho-providers",
            metadata=metadata,
            root=ROOT,
            version_available=lambda name, version: False,
            runner=runner,
        )
        self.assertEqual(
            commands[0][:6],
            (
                "cargo",
                "publish",
                "--dry-run",
                "--locked",
                "-p",
                "rho-providers",
            ),
        )
        self.assertIn("--config", commands[0])
        self.assertIn(
            'patch.crates-io.rho-sdk.path="crates/rho-sdk"',
            commands[0],
        )


class BoundaryFixtureTests(unittest.TestCase):
    # Covers: consumer importing an unpublished export fails without workspace patch
    # Owner: release packaging scripts

    def test_fixture_missing_export_fails_without_workspace_patch(self) -> None:
        prep.run_boundary_fixture(ROOT / "fixtures" / "publish-boundary")


if __name__ == "__main__":
    unittest.main()
