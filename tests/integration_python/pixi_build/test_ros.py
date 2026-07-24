"""Integration tests for the pixi-build-ros backend."""

import platform
from pathlib import Path

import pytest
from rattler.package_streaming import extract

from .common import ExitCode, copytree_with_local_backend, verify_cli_command

ROS_WORKSPACE_NAME = "ros-workspace"
ROS_PACKAGE_DIRS = ["navigator", "navigator_py", "distro_less_package", "pyproject_py"]
ROS_IMPLICIT_PACKAGE_DIR = "navigator_implicit"
ROS_PACKAGE_OUTPUT_NAMES = {
    "navigator": "ros-humble-navigator",
    "navigator_py": "ros-humble-navigator-py",
    # The `humble` distro is automatically selected from the channels in the pixi.toml
    "distro_less_package": "ros-humble-distro-less-package",
    # An ament_python package that only has a pyproject.toml (no setup.py)
    "pyproject_py": "ros-humble-pyproject-py",
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
        [pixi, "publish", "--target-dir", str(output_dir), "--path", manifest_path],
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
        [pixi, "publish", "--target-dir", str(output_dir), "--path", manifest_path],
    )

    expected_name = ROS_PACKAGE_OUTPUT_NAMES[package_dir]
    built_packages = list(output_dir.glob("*.conda"))
    assert built_packages, f"no package artifacts produced for {expected_name}"
    assert any(expected_name in artifact.name for artifact in built_packages)


@pytest.mark.slow
def test_ros_pyproject_package_contains_ament_index(
    pixi: Path, build_data: Path, tmp_pixi_workspace: Path
) -> None:
    """An ament_python package without a setup.py cannot register itself in the
    ament resource index, so the backend has to do it in the build script."""
    workspace = _prepare_ros_workspace(build_data, tmp_pixi_workspace)
    output_dir = workspace.joinpath("dist")
    output_dir.mkdir(parents=True, exist_ok=True)

    manifest_path = workspace.joinpath("src", "pyproject_py", "pixi.toml")

    verify_cli_command(
        [pixi, "publish", "--target-dir", str(output_dir), "--path", manifest_path],
    )

    built_packages = [
        artifact
        for artifact in output_dir.glob("*.conda")
        if "ros-humble-pyproject-py" in artifact.name
    ]
    assert built_packages, "no package artifact produced for ros-humble-pyproject-py"

    extract_dir = workspace.joinpath("extracted")
    extract(built_packages[0], extract_dir)

    prefix = extract_dir / "Library" if platform.system() == "Windows" else extract_dir
    assert (prefix / "share/ament_index/resource_index/packages/pyproject_py").is_file()
    assert (prefix / "share/pyproject_py/package.xml").is_file()
    # The console script must be discoverable by `ros2 run`, which looks in lib/<pkg>
    script_dir = prefix / "lib" / "pyproject_py"
    assert script_dir.is_dir()
    assert any(entry.name.startswith("navigate") for entry in script_dir.iterdir())


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
        [pixi, "publish", "--target-dir", str(output_dir), "--path", "package.xml"],
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
        [pixi, "publish", "--target-dir", str(output_dir), "--path", manifest_path],
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains=[
            "is a directory, please provide the path to the manifest file",
            "did you mean package.xml",
        ],
    )
