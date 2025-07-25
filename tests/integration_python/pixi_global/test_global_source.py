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
            "python_rich",  # this is the command name
        ],
        env=env,
    )

    # Check that the package was installed
    main = tmp_pixi_workspace / "bin" / exec_extension("rich-example-main")
    assert main.exists(), "`rich-example-main` executable was not created"


@pytest.mark.slow
def test_install_directory_cpp_project(
    pixi: Path,
    tmp_pixi_workspace: Path,
    test_data: Path,
) -> None:
    """Test installing a pixi project from a local directory with a C++ build."""
    # Make it one level deeper so that we do no pollute git with the global
    tmp_pixi_workspace = tmp_pixi_workspace / "global-env"
    env = {"PIXI_HOME": str(tmp_pixi_workspace)}

    # Use the simple C++ project
    source_project = test_data / "cpp_simple"

    # Test directory install
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--path",
            str(source_project),
            "simple_cpp",
        ],
        env=env,
    )

    # Check that the executable was installed
    executable = tmp_pixi_workspace / "bin" / exec_extension("simple_cpp")
    assert executable.exists(), "`simple_cpp` executable was not created"


@pytest.mark.slow
def test_add_git_repository_to_existing_environment(
    pixi: Path,
    tmp_pixi_workspace: Path,
    test_data: Path,
    git_test_repo: Callable[[Path, str], GitTestRepo],
) -> None:
    """Test adding a git-based source package to an existing global environment."""
    # Make it one level deeper so that we do no pollute git with the global
    tmp_pixi_workspace = tmp_pixi_workspace / "global-env"
    env = {"PIXI_HOME": str(tmp_pixi_workspace)}

    # First create a basic environment with a regular package
    verify_cli_command(
        [pixi, "global", "install", "--environment", "test_env", "python"],
        env=env,
    )

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

    # Test adding git package to existing environment
    verify_cli_command(
        [
            pixi,
            "global",
            "add",
            "--environment",
            "test_env",
            "--git",
            git_url,
            "python_rich",
            "--expose",
            "rich-example-main=rich-example-main"  # this is the command name
        ],
        env=env,
    )

    # Check that the package was added to the existing environment
    main = tmp_pixi_workspace / "bin" / exec_extension("rich-example-main")
    assert main.exists(), "`rich-example-main` executable was not created"
