from pathlib import Path

import pytest

from ..common import ExitCode, exec_extension, git_test_repo, verify_cli_command


@pytest.mark.slow
def test_install_multi_output(
    pixi: Path,
    tmp_path: Path,
    test_data: Path,
) -> None:
    """Test installing a pixi project from a git repository."""
    # Make it one level deeper so that we do no pollute git with the global
    pixi_home = tmp_path / "pixi_home"
    env = {"PIXI_HOME": str(pixi_home)}

    # Specify the project
    source_project = test_data.joinpath("pixi-build", "multi-output")

    # Test install without any specs mentioned
    # It should tell you which outputs are available
    verify_cli_command(
        [pixi, "global", "install", "--path", source_project],
        ExitCode.FAILURE,
        env=env,
        stderr_contains=["multiple outputs", "foobar", "bizbar", "foobar-desktop"],
    )

    # Test install and explicitly requesting `foobar-desktop`
    verify_cli_command(
        [pixi, "global", "install", "--path", source_project, "foobar-desktop"], env=env
    )

    # Check that the package was installed
    foobar_desktop = pixi_home / "bin" / exec_extension("foobar-desktop")
    assert foobar_desktop.exists(), "`foobar-desktop` executable was not created"


@pytest.mark.slow
@pytest.mark.parametrize("package_name", [None, "python_rich"])
def test_install_path_dependency_basic(
    pixi: Path,
    tmp_path: Path,
    test_data: Path,
    package_name: str | None,
) -> None:
    """Test installing a pixi project from a git repository."""
    # Make it one level deeper so that we do no pollute git with the global
    pixi_home = tmp_path / "pixi_home"
    env = {"PIXI_HOME": str(pixi_home)}

    # Specify the project
    source_project = test_data.joinpath("pixi-build", "simple-python")

    # Build command based on whether package name is provided
    cmd: list[str | Path] = [pixi, "global", "install", "--path", source_project]
    if package_name:
        cmd.append(package_name)

    # Test install
    verify_cli_command(cmd, env=env)

    # Check that the package was installed
    main = pixi_home / "bin" / exec_extension("rich-example-main")
    assert main.exists(), "`rich-example-main` executable was not created"


@pytest.mark.slow
@pytest.mark.parametrize("package_name", [None, "python_rich"])
def test_install_git_repository_basic(
    pixi: Path,
    tmp_path: Path,
    test_data: Path,
    package_name: str | None,
) -> None:
    """Test installing a pixi project from a git repository."""
    # Make it one level deeper so that we do no pollute git with the global
    pixi_home = tmp_path / "pixi_home"
    env = {"PIXI_HOME": str(pixi_home)}

    # Specify the project
    source_project = test_data.joinpath("pixi-build", "simple-python")

    # Create git repository
    git_url = git_test_repo(source_project, "test-project", tmp_path)

    # Build command based on whether package name is provided
    cmd: list[str | Path] = [pixi, "global", "install", "--git", git_url]
    if package_name:
        cmd.append(package_name)

    # Test git install
    verify_cli_command(cmd, env=env)

    # Check that the package was installed
    main = pixi_home / "bin" / exec_extension("rich-example-main")
    assert main.exists(), "`rich-example-main` executable was not created"


@pytest.mark.slow
def test_install_directory_cpp_project(
    pixi: Path,
    tmp_path: Path,
    test_data: Path,
) -> None:
    """Test installing a pixi project from a local directory with a C++ build."""
    # Make it one level deeper so that we do no pollute git with the global
    pixi_home = tmp_path / "pixi_home"
    env = {"PIXI_HOME": str(pixi_home)}

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
    executable = pixi_home / "bin" / exec_extension("simple_cpp")
    assert executable.exists(), "`simple_cpp` executable was not created"


@pytest.mark.slow
def test_add_git_repository_to_existing_environment(
    pixi: Path,
    tmp_path: Path,
    test_data: Path,
) -> None:
    """Test adding a git-based source package to an existing global environment."""
    # Make it one level deeper so that we do no pollute git with the global
    pixi_home = tmp_path / "pixi_home"
    env = {"PIXI_HOME": str(pixi_home)}

    # First create a basic environment with a regular package
    verify_cli_command(
        [pixi, "global", "install", "--environment", "test_env", "python"],
        env=env,
    )

    # Specify the source
    source_project = test_data.joinpath("pixi-build", "simple-python")

    # Create git repository
    git_url = git_test_repo(source_project, "test-project", tmp_path)

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
            "rich-example-main=rich-example-main",  # this is the command name
        ],
        env=env,
    )

    # Check that the package was added to the existing environment
    main = pixi_home / "bin" / exec_extension("rich-example-main")
    assert main.exists(), "`rich-example-main` executable was not created"
