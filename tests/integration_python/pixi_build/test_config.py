import platform
from pathlib import Path

import pytest

from .common import ExitCode, copytree_with_local_backend, get_manifest, verify_cli_command


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
