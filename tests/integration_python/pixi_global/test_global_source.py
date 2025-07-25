import tomllib
from pathlib import Path
from typing import Callable

import pytest
from ..common import verify_cli_command, exec_extension
from ..test_utils.git_utils import GitTestRepo

MANIFEST_VERSION = 1


@pytest.mark.slow
def test_install_git_repository_basic(
    pixi: Path,
    tmp_pixi_workspace: Path,
    test_data: Path,
    git_test_repo: Callable[[Path, str], GitTestRepo],
) -> None:
    """Test installing a pixi project from a git repository."""
    # Make it one level deeper so that we do no pollute git with the global
    tmp_pixi_workspace = tmp_pixi_workspace / "global-env"
    env = {"PIXI_HOME": str(tmp_pixi_workspace)}

    # Use the test-project-export as our source
    source_project = (
        test_data
        / ".."
        / ".."
        / "docs"
        / "source_files"
        / "pixi_workspaces"
        / "pixi_build"
        / "python"
    )

    # Create git repository and start daemon
    git_repo = git_test_repo(source_project, "test-project")
    git_url = git_repo.get_git_url()

    # Test git install
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--git",
            git_url,
            "main",  # this is the command name
        ],
        env=env,
    )

    # Check that the package was installed
    main = tmp_pixi_workspace / "bin" / exec_extension("main")
    assert main.exists(), "Main executable was not created"


@pytest.mark.skip
@pytest.mark.slow
def test_install_git_repository_with_custom_environment(
    pixi: Path,
    tmp_pixi_workspace: Path,
    test_data: Path,
    git_test_repo: Callable[[Path, str], GitTestRepo],
) -> None:
    """Test installing from git with custom environment name."""
    env = {"PIXI_HOME": str(tmp_pixi_workspace)}

    # Use the simple project as our source
    source_project = test_data / "discovery" / "simple"

    # Create git repository and start daemon
    git_repo = git_test_repo(source_project, "simple-project")
    git_url = git_repo.get_git_url()

    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--git",
            git_url,
            "--branch",
            "main",
            "--environment",
            "custom-simple",
        ],
        env=env,
    )

    # Check manifest was updated with custom environment name
    manifest = tmp_pixi_workspace / "manifests" / "pixi-global.toml"
    parsed_toml = tomllib.loads(manifest.read_text())
    assert "custom-simple" in parsed_toml["envs"]


@pytest.mark.skip
@pytest.mark.slow
def test_install_git_repository_with_expose(
    pixi: Path,
    tmp_pixi_workspace: Path,
    test_data: Path,
    git_test_repo: Callable[[Path, str], GitTestRepo],
) -> None:
    """Test installing from git with custom expose settings."""
    env = {"PIXI_HOME": str(tmp_pixi_workspace)}

    # Use the test-project-export as our source
    source_project = test_data / "mock-projects" / "test-project-export"

    # Create git repository and start daemon
    git_repo = git_test_repo(source_project, "expose-test")
    git_url = git_repo.get_git_url()

    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--git",
            git_url,
            "--branch",
            "main",
            "--expose",
            "custom-binary=some-binary",
        ],
        env=env,
    )

    # Check manifest has custom expose mapping
    manifest = tmp_pixi_workspace / "manifests" / "pixi-global.toml"
    parsed_toml = tomllib.loads(manifest.read_text())

    # Should have an environment with custom expose
    assert len(parsed_toml["envs"]) >= 1
    env_name = list(parsed_toml["envs"].keys())[0]
    if "exposed" in parsed_toml["envs"][env_name]:
        assert "custom-binary" in parsed_toml["envs"][env_name]["exposed"]
