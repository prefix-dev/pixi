from pathlib import Path
from .common import verify_cli_command, ExitCode

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


def test_run_with_activation(pixi: Path, tmp_path: Path) -> None:
    manifest = tmp_path.joinpath("pixi.toml")
    toml = f"""
    {EMPTY_BOILERPLATE_PROJECT}
    [activation.env]
    TEST_ENV_VAR = "test123"
    [tasks]
    task = "echo $TEST_ENV_VAR"
    """
    manifest.write_text(toml)
    # Run the default task
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "task"],
        ExitCode.SUCCESS,
        stdout_contains="test123",
    )

    # Modify the environment variable in cache
    cache_path = tmp_path.joinpath(".pixi/activation-env-v0/activation_default.json")
    with cache_path.open("r+") as f:
        contents = f.read()
        new_contents = contents.replace("test123", "test456")
        f.seek(0)  # Move pointer to start of the file
        f.write(new_contents)
        f.truncate()  # Remove any remaining original content

    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "task", "-vvv"],
        ExitCode.SUCCESS,
        # Contain overwritten value
        stdout_contains="test456",
        stdout_excludes="test123",
    )

    # Ignore activation cache
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "--no-env-activation-cache", "task", "-vvv"],
        ExitCode.SUCCESS,
        stdout_contains="test123",
    )
