"""Integration tests for the pixi-build-ros backend."""

from pathlib import Path

import pytest

from .common import ExitCode, copytree_with_local_backend, verify_cli_command

ROS_WORKSPACE_NAME = "ros-workspace"
ROS_PACKAGE_DIRS = ["navigator", "navigator_py", "distro_less_package"]
ROS_IMPLICIT_PACKAGE_DIR = "navigator_implicit"
ROS_PACKAGE_OUTPUT_NAMES = {
    "navigator": "ros-humble-navigator",
    "navigator_py": "ros-humble-navigator-py",
    # The `humble` distro is automatically selected from the channels in the pixi.toml
    "distro_less_package": "ros-humble-distro-less-package",
}


def _prepare_ros_workspace(build_data: Path, tmp_pixi_workspace: Path) -> Path:
    workspace_src = build_data.joinpath(ROS_WORKSPACE_NAME)
    copytree_with_local_backend(workspace_src, tmp_pixi_workspace, dirs_exist_ok=True)
    return tmp_pixi_workspace


@pytest.mark.slow
@pytest.mark.parametrize("package_dir", ROS_PACKAGE_DIRS, ids=ROS_PACKAGE_DIRS)
def test_ros_packages_build(
    package_dir: str, pixi: Path, build_data: Path, tmp_pixi_workspace: Path
) -> None:
    workspace = _prepare_ros_workspace(build_data, tmp_pixi_workspace)
    output_dir = workspace.joinpath("dist")
    output_dir.mkdir(parents=True, exist_ok=True)

    # point to the concrete package manifest
    manifest_path = workspace.joinpath("src", package_dir, "pixi.toml")

    verify_cli_command(
        [pixi, "publish", str(output_dir), "--path", manifest_path],
    )

    expected_name = ROS_PACKAGE_OUTPUT_NAMES[package_dir]
    built_packages = list(output_dir.glob("*.conda"))
    assert built_packages, f"no package artifacts produced for {expected_name}"
    assert any(expected_name in artifact.name for artifact in built_packages)


@pytest.mark.slow
@pytest.mark.parametrize("package_dir", ROS_PACKAGE_DIRS, ids=ROS_PACKAGE_DIRS)
def test_ros_packages_build_point_to_package_xml(
    package_dir: str, pixi: Path, build_data: Path, tmp_pixi_workspace: Path
) -> None:
    workspace = _prepare_ros_workspace(build_data, tmp_pixi_workspace)
    output_dir = workspace.joinpath("dist")
    output_dir.mkdir(parents=True, exist_ok=True)

    # point to the concrete package manifest
    manifest_path = workspace.joinpath("src", package_dir, "package.xml")

    verify_cli_command(
        [pixi, "publish", str(output_dir), "--path", manifest_path],
    )

    expected_name = ROS_PACKAGE_OUTPUT_NAMES[package_dir]
    built_packages = list(output_dir.glob("*.conda"))
    assert built_packages, f"no package artifacts produced for {expected_name}"
    assert any(expected_name in artifact.name for artifact in built_packages)


@pytest.mark.slow
def test_ros_packages_build_point_to_package_xml_in_the_same_dir(
    pixi: Path, build_data: Path, tmp_pixi_workspace: Path
) -> None:
    workspace = _prepare_ros_workspace(build_data, tmp_pixi_workspace)
    output_dir = workspace.joinpath("dist")
    output_dir.mkdir(parents=True, exist_ok=True)

    # point to the concrete package manifest
    manifest_path = workspace.joinpath("src", ROS_IMPLICIT_PACKAGE_DIR)

    verify_cli_command(
        [pixi, "publish", str(output_dir), "--path", "package.xml"],
        cwd=manifest_path,
    )

    built_packages = list(output_dir.glob("*.conda"))
    assert built_packages, "no package artifacts produced"


@pytest.mark.slow
def test_ros_packages_build_point_to_implicit_package_xml_fails(
    pixi: Path, build_data: Path, tmp_pixi_workspace: Path
) -> None:
    workspace = _prepare_ros_workspace(build_data, tmp_pixi_workspace)
    output_dir = workspace.joinpath("dist")
    output_dir.mkdir(parents=True, exist_ok=True)

    # point to the concrete package manifest
    manifest_path = workspace.joinpath("src", ROS_IMPLICIT_PACKAGE_DIR)

    verify_cli_command(
        [pixi, "publish", str(output_dir), "--path", manifest_path],
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains=[
            "is a directory, please provide the path to the manifest file",
            "did you mean package.xml",
        ],
    )
