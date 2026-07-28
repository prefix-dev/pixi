"""Integration tests for the pixi-build-ros backend."""

from pathlib import Path

import pytest

from .common import ExitCode, copytree_with_local_backend, package_files, verify_cli_command

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


def _publish(pixi: Path, manifest_path: Path, output_dir: Path) -> Path:
    """Publish the package and return its single artifact."""
    output_dir.mkdir(parents=True, exist_ok=True)
    verify_cli_command(
        [pixi, "publish", "--target-dir", str(output_dir), "--path", manifest_path],
    )
    built_packages = list(output_dir.glob("*.conda"))
    assert len(built_packages) == 1, f"expected a single artifact, got {built_packages}"
    return built_packages[0]


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


@pytest.mark.slow
def test_ros_cmake_rebuild_omits_removed_file(
    pixi: Path, build_data: Path, tmp_pixi_workspace: Path
) -> None:
    workspace = _prepare_ros_workspace(build_data, tmp_pixi_workspace)
    package_dir = workspace.joinpath("src", "navigator")
    manifest_path = package_dir.joinpath("pixi.toml")
    cmakelists = package_dir.joinpath("CMakeLists.txt")
    original_cmakelists = cmakelists.read_text()
    stale_path = "share/navigator/stale.txt"

    # Install an extra file that the rebuild below takes away again.
    package_dir.joinpath("stale.txt").write_text("stale\n")
    cmakelists.write_text(
        original_cmakelists.replace(
            "ament_package()",
            "install(FILES stale.txt DESTINATION share/${PROJECT_NAME})\n\nament_package()",
        )
    )
    first_package = _publish(pixi, manifest_path, workspace.joinpath("dist-first"))
    assert stale_path in package_files(first_package)

    # `cmake --install` leaves the file behind in the reused prefix, so the second
    # package should only contain what this build installed.
    cmakelists.write_text(original_cmakelists)
    package_dir.joinpath("stale.txt").unlink()
    second_package = _publish(pixi, manifest_path, workspace.joinpath("dist-second"))
    assert stale_path not in package_files(second_package)


@pytest.mark.slow
def test_ros_python_rebuild_omits_removed_module(
    pixi: Path, build_data: Path, tmp_pixi_workspace: Path
) -> None:
    workspace = _prepare_ros_workspace(build_data, tmp_pixi_workspace)
    package_dir = workspace.joinpath("src", "navigator_py")
    manifest_path = package_dir.joinpath("pixi.toml")
    # `find_packages` picks this up without touching setup.py.
    stale_module = package_dir.joinpath("navigator_py", "stale.py")

    stale_module.write_text("STALE = True\n")
    first_package = _publish(pixi, manifest_path, workspace.joinpath("dist-first"))
    assert any(path.endswith("navigator_py/stale.py") for path in package_files(first_package)), (
        "the module to remove was never packaged"
    )

    # `setup.py install` leaves the module behind in the reused prefix, so the second
    # package should only contain what this build installed.
    stale_module.unlink()
    second_package = _publish(pixi, manifest_path, workspace.joinpath("dist-second"))
    assert not [path for path in package_files(second_package) if "stale" in path]
