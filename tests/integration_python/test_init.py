from pathlib import Path

import pytest

from .common import ExitCode, verify_cli_command


def test_pixi_init_cwd(pixi: Path, tmp_pixi_workspace: Path) -> None:
    # Create a new project
    verify_cli_command([pixi, "init", "."], ExitCode.SUCCESS, cwd=tmp_pixi_workspace)

    # Verify that the manifest file is created
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    assert manifest_path.exists()

    # Verify that the manifest file contains expected content
    manifest_content = manifest_path.read_text()
    assert "[workspace]" in manifest_content


def test_pixi_init_non_existing_dir(pixi: Path, tmp_pixi_workspace: Path) -> None:
    # Specify project dir
    project_dir = tmp_pixi_workspace / "project_dir"

    # Create a new project
    verify_cli_command([pixi, "init", project_dir], ExitCode.SUCCESS)

    # Verify that the manifest file is created
    manifest_path = project_dir / "pixi.toml"
    assert manifest_path.exists()

    # Verify that the manifest file contains expected content
    manifest_content = manifest_path.read_text()
    assert "[workspace]" in manifest_content


def test_pixi_init_pixi_home_parent(pixi: Path, tmp_pixi_workspace: Path) -> None:
    pixi_home = tmp_pixi_workspace / ".pixi"
    pixi_home.mkdir(exist_ok=True)

    verify_cli_command(
        [pixi, "init", pixi_home.parent],
        ExitCode.FAILURE,
        # Test that we print a helpful error message
        stderr_contains="pixi init",
        env={"PIXI_HOME": str(pixi_home)},
    )


@pytest.mark.slow
def test_pixi_init_pyproject(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest_path = tmp_pixi_workspace / "pyproject.toml"
    # Create a new project
    verify_cli_command(
        [pixi, "init", tmp_pixi_workspace, "--format", "pyproject"], ExitCode.SUCCESS
    )
    # Verify that install works
    verify_cli_command([pixi, "install", "--manifest-path", manifest_path], ExitCode.SUCCESS)
