import json
from pathlib import Path
from typing import Any

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


def _load_package_metadata(project_root: Path, package_name: str) -> dict[str, Any]:
    metadata_root = project_root.joinpath(".pixi", "build", "metadata-v0")
    assert metadata_root.exists(), f"metadata directory missing for {package_name}"
    selected_metadata: dict[str, Any] | None = None
    selected_mtime: float = -1.0
    for metadata_file in metadata_root.rglob("metadata.json"):
        metadata = json.loads(metadata_file.read_text())
        outputs = metadata.get("outputs", [])
        if any(
            isinstance(output, dict) and output.get("metadata", {}).get("name") == package_name
            for output in outputs
        ):
            mtime = metadata_file.stat().st_mtime
            if mtime > selected_mtime:
                selected_metadata = metadata
                selected_mtime = mtime
    if selected_metadata is not None:
        return selected_metadata
    raise AssertionError(f"metadata for {package_name} not found")


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
        [
            pixi,
            "build",
            "--path",
            manifest_path,
            "--output-dir",
            output_dir,
        ]
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
        [
            pixi,
            "build",
            "--path",
            manifest_path,
            "--output-dir",
            output_dir,
        ]
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
        [
            pixi,
            "build",
            "--path",
            "package.xml",
            "--output-dir",
            output_dir,
        ],
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
        [
            pixi,
            "build",
            "--path",
            manifest_path,
            "--output-dir",
            output_dir,
        ],
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains=[
            "is a directory, please provide the path to the manifest file",
            "did you mean package.xml",
        ],
    )
