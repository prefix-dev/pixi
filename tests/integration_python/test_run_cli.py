from pathlib import Path
import pytest
from .common import verify_cli_command, ExitCode, default_env_path

ALL_PLATFORMS = '["linux-64", "osx-64", "win-64", "linux-ppc64le", "linux-aarch64"]'

EMPTY_BOILERPLATE_PROJECT = f"""
[project]
name = "test"
channels = []
platforms = {ALL_PLATFORMS}
"""


def test_run_in_shell(pixi: Path, tmp_path: Path) -> None:
    manifest = tmp_path.joinpath("pixi.toml")
    toml = f"""
    {EMPTY_BOILERPLATE_PROJECT}
    [tasks]
    task = "echo default"
    task1 = "echo default1"
    [feature.a.tasks]
    task = {{ cmd = "echo a", depends-on = "task1" }}
    task1 = "echo a1"

    [environments]
    a = ["a"]
    """
    manifest.write_text(toml)

    # Run the default task
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "--environment", "default", "task"],
        ExitCode.SUCCESS,
        stdout_contains="default",
        stderr_excludes="default1",
    )

    # Run the a task
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "--environment", "a", "task"],
        ExitCode.SUCCESS,
        stdout_contains=["a", "a1"],
    )

    # Error on non-specified environment as ambiguous
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "task"],
        ExitCode.FAILURE,
        stderr_contains=["ambiguous", "default", "a"],
    )

    # Simulate activated shell in environment 'a'
    env = {"PIXI_IN_SHELL": "true", "PIXI_ENVIRONMENT_NAME": "a"}
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "task"],
        ExitCode.SUCCESS,
        stdout_contains=["a", "a1"],
        env=env,
    )


def test_using_prefix_validation(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    manifest = tmp_path.joinpath("pixi.toml")
    toml = f"""
    [project]
    name = "test"
    channels = ["{dummy_channel_1}"]
    platforms = ["linux-64", "osx-64", "osx-arm64", "win-64"]

    [dependencies]
    dummy-a = "*"
    """
    manifest.write_text(toml)

    # Run the install
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
    )

    # Validate creation of the pixi file with the hash
    pixi_file = default_env_path(tmp_path).joinpath("conda-meta").joinpath("pixi")
    assert pixi_file.exists()
    assert "environment_lock_file_hash" in pixi_file.read_text()

    # Break environment on purpose
    dummy_a_meta_files = default_env_path(tmp_path).joinpath("conda-meta").glob("dummy-a*.json")

    for file in dummy_a_meta_files:
        path = Path(file)
        if path.exists():
            path.unlink()  # Removes the file

    # Run simple script, which shouldn't reinstall
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "echo", "hello"],
        stdout_contains="hello",
    )

    # Validate that the dummy-a files still don't exist
    for file in dummy_a_meta_files:
        assert not Path(file).exists()

    # Run an actual re-install
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
    )

    # Validate the files are back
    for file in dummy_a_meta_files:
        # All dummy-a files should be back as `install` will ignore the hash
        assert Path(file).exists()


def test_prefix_revalidation(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    manifest = tmp_path.joinpath("pixi.toml")
    toml = f"""
    [project]
    name = "test"
    channels = ["{dummy_channel_1}"]
    platforms = ["linux-64", "osx-64", "osx-arm64", "win-64"]

    [dependencies]
    dummy-a = "*"
    """
    manifest.write_text(toml)

    # Run the installation
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
        ExitCode.SUCCESS,
    )

    # Validate creation of the pixi file with the hash
    pixi_file = default_env_path(tmp_path).joinpath("conda-meta").joinpath("pixi")
    assert pixi_file.exists()
    assert "environment_lock_file_hash" in pixi_file.read_text()

    # Break environment on purpose
    dummy_a_meta_files = default_env_path(tmp_path).joinpath("conda-meta").glob("dummy-a*.json")

    for file in dummy_a_meta_files:
        path = Path(file)
        if path.exists():
            path.unlink()  # Removes the file

    # Run with revalidation to force reinstallation
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "--revalidate", "echo", "hello"],
        ExitCode.SUCCESS,
        stdout_contains="hello",
    )

    # Validate that the dummy-a files are reinstalled
    for file in dummy_a_meta_files:
        assert Path(file).exists()


# only run on linux
@pytest.mark.slow
def test_pypi_git_deps(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    test_data = Path(__file__).parent.parent / "data/pixi_tomls/pip_git_dep.toml"
    manifest = tmp_path.joinpath("pixi.toml")
    toml = test_data.read_text()
    manifest.write_text(toml)

    # Run the installation
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
        ExitCode.SUCCESS,
    )
