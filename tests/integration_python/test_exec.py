import sys
from concurrent.futures import ProcessPoolExecutor, as_completed
from pathlib import Path

import pytest

from .common import ExitCode, verify_cli_command, run_and_get_env


@pytest.mark.skipif(
    sys.platform.startswith("win"),
    reason="For some reason .bat files are not correctly executed on windows",
)
def test_concurrent_exec(pixi: Path, dummy_channel_1: str) -> None:
    with ProcessPoolExecutor(max_workers=2) as executor:
        # Run the two exact same tasks in parallel
        futures = [
            executor.submit(
                verify_cli_command,
                [pixi, "exec", "-c", dummy_channel_1, "dummy-f"],
                stdout_contains=["dummy-f on"],
            ),
            executor.submit(
                verify_cli_command,
                [pixi, "exec", "-c", dummy_channel_1, "dummy-f"],
                stdout_contains=["dummy-f on"],
            ),
        ]

        # Ensure both tasks are actually running in parallel and wait for them to finish
        for future in as_completed(futures):
            future.result()


@pytest.mark.skipif(
    sys.platform.startswith("win"),
    reason="For some reason .bat files are not correctly executed on windows",
)
def test_exec_list(pixi: Path, dummy_channel_1: str) -> None:
    # Without `--list`, nothing is listed
    verify_cli_command(
        [pixi, "exec", "--channel", dummy_channel_1, "dummy-g"],
        stdout_excludes=["dummy-g"],
    )

    # List all packages in environment
    verify_cli_command(
        [pixi, "exec", "--channel", dummy_channel_1, "--list", "dummy-g"],
        stdout_contains=["dummy-g", "dummy-b"],
    )

    # List only packages that match regex "g"
    verify_cli_command(
        [pixi, "exec", "--channel", dummy_channel_1, "--list=g", "dummy-g"],
        stdout_contains="dummy-g",
        stdout_excludes="dummy-b",
    )

    # List specific package
    verify_cli_command(
        [pixi, "exec", "--channel", dummy_channel_1, "--list=dummy-g", "dummy-g"],
        stdout_contains=["dummy-g"],
        stdout_excludes=["dummy-b"],
    )


def test_pixi_environment_name_and_ps1(pixi: Path, dummy_channel_1: str) -> None:
    """Test that PIXI_ENVIRONMENT_NAME and PS1/PROMPT are set correctly."""
    # Test with single package
    env_value, _ = run_and_get_env(
        pixi, "--channel", dummy_channel_1, "-s", "dummy-a", env_var="PIXI_ENVIRONMENT_NAME"
    )
    assert env_value == "temp:dummy-a"

    # Test with multiple packages (should be sorted)
    env_value, _ = run_and_get_env(
        pixi,
        "--channel",
        dummy_channel_1,
        "-s",
        "dummy-c",
        "-s",
        "dummy-a",
        env_var="PIXI_ENVIRONMENT_NAME",
    )
    assert env_value == "temp:dummy-a,dummy-c"

    # Test with --with flag
    env_value, _ = run_and_get_env(
        pixi,
        "--channel",
        dummy_channel_1,
        "--with",
        "dummy-b",
        "--with",
        "dummy-c",
        env_var="PIXI_ENVIRONMENT_NAME",
    )
    assert env_value == "temp:dummy-b,dummy-c"

    # Test with no specs (should not set the variable)
    env_value, _ = run_and_get_env(
        pixi, "--channel", dummy_channel_1, env_var="PIXI_ENVIRONMENT_NAME"
    )
    assert env_value is None

    # Test PS1 modification
    if sys.platform.startswith("win"):
        prompt_var = "_PIXI_PROMPT"
        expected_prompt = "(pixi:temp:dummy-a) $P$G"
    else:
        prompt_var = "PS1"
        expected_prompt = r"(pixi:temp:dummy-a) [\w] \$"

    # Test with default behavior (prompt should be modified)
    prompt, _ = run_and_get_env(
        pixi, "--channel", dummy_channel_1, "-s", "dummy-a", env_var=prompt_var
    )
    assert prompt == expected_prompt

    # Test with --no-modify-ps1 (prompt should not be modified)
    prompt, _ = run_and_get_env(
        pixi,
        "--channel",
        dummy_channel_1,
        "--no-modify-ps1",
        "-s",
        "dummy-a",
        env_var=prompt_var,
    )
    if sys.platform.startswith("win"):
        assert prompt is None
    else:
        assert prompt is None or "(pixi:temp:dummy-a)" not in prompt


@pytest.mark.skipif(
    sys.platform.startswith("win"),
    reason="For some reason .bat files are not correctly executed on windows",
)
def test_exec_with(pixi: Path, dummy_channel_1: str) -> None:
    # A package is guessed from the command when `--with` is provided
    verify_cli_command(
        [pixi, "exec", "--channel", dummy_channel_1, "--list", "--spec=dummy-a", "dummy-f"],
        stdout_excludes="dummy-f",
        expected_exit_code=ExitCode.FAILURE,
    )
    verify_cli_command(
        [pixi, "exec", "--channel", dummy_channel_1, "--list", "--with=dummy-a", "dummy-f"],
        stdout_contains="dummy-f",
    )

    # Correct behaviour with multiple 'with' options
    verify_cli_command(
        [
            pixi,
            "exec",
            "--channel",
            dummy_channel_1,
            "--list",
            "--with=dummy-a",
            "--with=dummy-b",
            "dummy-f",
        ],
        stdout_contains=["dummy-a", "dummy-b", "dummy-f"],
    )

    # 'with' and 'spec' options mutually exclusive
    verify_cli_command(
        [
            pixi,
            "exec",
            "--channel",
            dummy_channel_1,
            "--list",
            "--with=dummy-a",
            "--spec=dummy-b",
            "dummy-f",
        ],
        expected_exit_code=ExitCode.INCORRECT_USAGE,
        stderr_contains="cannot be used with",
    )
