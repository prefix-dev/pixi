import json
import platform
import tomllib
from pathlib import Path

import pytest
import tomli_w

from .common import (
    CURRENT_PLATFORM,
    ExitCode,
    copytree_with_local_backend,
    default_env_path,
    get_manifest,
    verify_cli_command,
)


@pytest.mark.slow
@pytest.mark.parametrize("changed_input", ["version", "configuration", "target_configuration"])
def test_artifact_cache_tracks_package_and_build_settings(
    pixi: Path, tmp_pixi_workspace: Path, build_data: Path, changed_input: str
) -> None:
    copytree_with_local_backend(
        build_data / "artifact-cache-inputs", tmp_pixi_workspace, dirs_exist_ok=True
    )
    manifest = get_manifest(tmp_pixi_workspace)
    command = [pixi, "install", "--manifest-path", manifest]
    prefix = default_env_path(tmp_pixi_workspace)
    marker_file = prefix / "share/artifact-key-repro/repro-value.txt"

    def assert_installed(version: str, value: str) -> None:
        records = list((prefix / "conda-meta").glob("artifact-key-repro-*.json"))
        assert len(records) == 1
        assert json.loads(records[0].read_text())["version"] == version
        assert marker_file.read_text().strip() == value

    verify_cli_command(command)
    assert_installed("0.1.0", "before")

    manifest_data = tomllib.loads(manifest.read_text())
    expected_version, expected_value = "0.1.0", "before"
    if changed_input == "version":
        manifest_data["package"]["version"] = expected_version = "0.2.0"
    elif changed_input == "configuration":
        manifest_data["package"]["build"]["config"]["env"]["REPRO_VALUE"] = expected_value = "after"
    else:
        manifest_data["package"]["build"]["target"] = {
            CURRENT_PLATFORM: {"config": {"env": {"REPRO_VALUE": "after"}}}
        }
        expected_value = "after"
    manifest.write_text(tomli_w.dumps(manifest_data))

    # Leave the artifact cache and installed environment intact.
    verify_cli_command(command)
    assert_installed(expected_version, expected_value)

    # The updated artifact must remain reusable on an unchanged invocation.
    verify_cli_command(command, stderr_excludes="Running build for recipe:")
    assert_installed(expected_version, expected_value)


@pytest.mark.slow
def test_pixi_build_cmake_env_config_without_target(
    pixi: Path, tmp_pixi_workspace: Path, build_data: Path
) -> None:
    """Test that env configuration without target specific configuration works correctly with pixi-build-cmake backend."""

    # Copy the cmake env config test workspace
    cmake_env_test_project = build_data.joinpath("env-config-cmake-test")

    # Copy to workspace
    copytree_with_local_backend(cmake_env_test_project, tmp_pixi_workspace, dirs_exist_ok=True)

    # Get manifest
    manifest = get_manifest(tmp_pixi_workspace)

    # Install the package - this should show env vars in the build output
    verify_cli_command(
        [pixi, "install", "-v", "--manifest-path", manifest],
        stderr_contains=[
            "CUSTOM_BUILD_VAR=test_value",
            "PIXI_TEST_ENV=pixi_cmake_test",
            "BUILD_MESSAGE=hello_from_env",
        ],
    )


@pytest.mark.slow
def test_pixi_build_cmake_env_config_with_target(
    pixi: Path, tmp_pixi_workspace: Path, build_data: Path
) -> None:
    """Test that target-specific env configuration works correctly with pixi-build-cmake backend."""

    # Copy the target cmake env config test workspace
    cmake_target_env_test_project = build_data.joinpath("env-config-target-cmake-test")

    # Copy to workspace
    copytree_with_local_backend(
        cmake_target_env_test_project, tmp_pixi_workspace, dirs_exist_ok=True
    )

    # Get manifest
    manifest = get_manifest(tmp_pixi_workspace)

    # Platform-specific expectations
    current_sys = platform.system().lower()

    if current_sys == "windows":
        # On Windows, expect win-64 specific variables
        verify_cli_command(
            [pixi, "install", "-v", "--manifest-path", manifest],
            stderr_contains=[
                "GLOBAL_ENV_VAR=global_value",
                "WIN_SPECIFIC_VAR=windows_value",
                "PLATFORM_TYPE=win-64",
            ],
        )
    else:
        # On Unix-like systems (Linux, macOS), expect unix specific variables
        verify_cli_command(
            [pixi, "install", "-v", "--manifest-path", manifest],
            stderr_contains=[
                "GLOBAL_ENV_VAR=global_value",
                "UNIX_SPECIFIC_VAR=unix_value",
                "PLATFORM_TYPE=unix",
            ],
        )


@pytest.mark.slow
def test_pixi_build_cmake_invalid_config_rejection(
    pixi: Path, tmp_pixi_workspace: Path, build_data: Path
) -> None:
    """Test that invalid configuration keys are rejected."""

    # Copy the invalid config test workspace
    cmake_invalid_test_project = build_data.joinpath("env-config-invalid-test")

    # Copy to workspace
    copytree_with_local_backend(cmake_invalid_test_project, tmp_pixi_workspace, dirs_exist_ok=True)

    # Get manifest
    manifest = get_manifest(tmp_pixi_workspace)

    # Install should fail due to invalid configuration key
    verify_cli_command(
        [pixi, "install", "-v", "--manifest-path", manifest],
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains=[
            "failed to parse configuration",
            "unknown field `invalid_config_key`",
        ],
    )


@pytest.mark.slow
def test_pixi_build_cmake_invalid_target_config_rejection(
    pixi: Path, tmp_pixi_workspace: Path, build_data: Path
) -> None:
    """Test that invalid target-specific configuration keys are rejected."""

    # Copy the invalid target config test workspace
    cmake_target_invalid_test_project = build_data.joinpath("env-config-target-invalid-test")

    # Copy to workspace
    copytree_with_local_backend(
        cmake_target_invalid_test_project, tmp_pixi_workspace, dirs_exist_ok=True
    )

    # Get manifest
    manifest = get_manifest(tmp_pixi_workspace)

    # Install should fail due to invalid target configuration key
    verify_cli_command(
        [pixi, "install", "-v", "--manifest-path", manifest],
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains=[
            "failed to parse target configuration",
            "unknown field `invalid_target_config_key`",
        ],
    )
