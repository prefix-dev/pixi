import json
from pathlib import Path
from .common import verify_cli_command, ExitCode, default_env_path
import tempfile
import os

ALL_PLATFORMS = '["linux-64", "osx-64", "win-64", "linux-ppc64le", "linux-aarch64"]'

EMPTY_BOILERPLATE_PROJECT = f"""
[project]
name = "test"
channels = []
platforms = {ALL_PLATFORMS}
"""


def test_run_in_shell_environment(pixi: Path, tmp_path: Path) -> None:
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
        stdout_contains="default",
        stderr_excludes="default1",
    )

    # Run the a task
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "--environment", "a", "task"],
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
        stdout_contains=["a", "a1"],
        env=env,
    )


def test_run_in_shell_project(pixi: Path) -> None:
    # We don't want a `pixi.toml` in our parent directory
    # so let's use tempfile here
    with tempfile.TemporaryDirectory() as tmp_str:
        tmp_path = Path(tmp_str)
        manifest_1_dir = tmp_path.joinpath("manifest_1")
        manifest_1_dir.mkdir()
        manifest_1 = manifest_1_dir.joinpath("pixi.toml")
        toml = f"""
        {EMPTY_BOILERPLATE_PROJECT}
        [tasks]
        task = "echo manifest_1"
        """
        manifest_1.write_text(toml)

        manifest_2_dir = tmp_path.joinpath("manifest_2")
        manifest_2_dir.mkdir()
        manifest_2 = manifest_2_dir.joinpath("pixi.toml")
        toml = f"""
        {EMPTY_BOILERPLATE_PROJECT}
        [tasks]
        task = "echo manifest_2"
        """
        manifest_2.write_text(toml)

        base_env = dict(os.environ)
        base_env.pop("PIXI_IN_SHELL", None)
        base_env.pop("PIXI_PROJECT_MANIFEST", None)
        extended_env = base_env | {
            "PIXI_IN_SHELL": "true",
            "PIXI_PROJECT_MANIFEST": str(manifest_2),
        }

        # Run task with PIXI_PROJECT_MANIFEST set to manifest_2
        verify_cli_command(
            [pixi, "run", "task"],
            stdout_contains="manifest_2",
            env=extended_env,
            cwd=tmp_path,
            reset_env=True,
        )

        # Run with working directory at manifest_1_dir
        verify_cli_command(
            [pixi, "run", "task"],
            stdout_contains="manifest_1",
            env=base_env,
            cwd=manifest_1_dir,
            reset_env=True,
        )

        # Run task with PIXI_PROJECT_MANIFEST set to manifest_2 and working directory at manifest_1_dir
        # working directory should win
        # pixi should warn that it uses the local manifest rather than PIXI_PROJECT_MANIFEST
        verify_cli_command(
            [pixi, "run", "task"],
            stdout_contains="manifest_1",
            stderr_contains="manifest_2",
            env=extended_env,
            cwd=manifest_1_dir,
            reset_env=True,
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
        stdout_contains="hello",
    )

    # Validate that the dummy-a files are reinstalled
    for file in dummy_a_meta_files:
        assert Path(file).exists()


def test_run_with_activation(pixi: Path, tmp_path: Path) -> None:
    manifest = tmp_path.joinpath("pixi.toml")
    toml = f"""
    {EMPTY_BOILERPLATE_PROJECT}
    [activation.env]
    TEST_ENV_VAR_FOR_ACTIVATION_TEST = "test123"
    [tasks]
    task = "echo $TEST_ENV_VAR_FOR_ACTIVATION_TEST"
    """
    manifest.write_text(toml)

    # Run the default task
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "task"],
        stdout_contains="test123",
    )

    # Validate that without experimental it does not use the cache
    assert not tmp_path.joinpath(".pixi/activation-env-v0").exists()

    # Enable the experimental cache config
    verify_cli_command(
        [
            pixi,
            "config",
            "set",
            "--manifest-path",
            manifest,
            "--local",
            "experimental.use-environment-activation-cache",
            "true",
        ],
    )

    # Run the default task and create cache
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "task"],
        stdout_contains="test123",
    )

    # Modify the environment variable in cache
    cache_path = tmp_path.joinpath(".pixi", "activation-env-v0", "activation_default.json")
    data = json.loads(cache_path.read_text())
    data["environment_variables"]["TEST_ENV_VAR_FOR_ACTIVATION_TEST"] = "test456"
    cache_path.write_text(json.dumps(data, indent=4))

    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "task"],
        # Contain overwritten value
        stdout_contains="test456",
        stdout_excludes="test123",
    )

    # Ignore activation cache
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "--force-activate", "task", "-vvv"],
        stdout_contains="test123",
    )
