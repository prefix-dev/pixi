import sys
from concurrent.futures import ProcessPoolExecutor, as_completed
from pathlib import Path

import pytest

from .common import ExitCode, verify_cli_command


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


@pytest.mark.skipif(
    sys.platform.startswith("win"),
    reason="For some reason .bat files are not correctly executed on windows",
)
def test_exec_with(pixi: Path, dummy_channel_1: str) -> None:
    # A package is guessed from the command when `--with` is provided
    verify_cli_command(
        [pixi, "exec", "--channel", dummy_channel_1, "--list", "--spec=dummy-a", "dummy-b"],
        stdout_excludes="dummy-b",
        expected_exit_code=ExitCode.FAILURE,
    )
    verify_cli_command(
        [pixi, "exec", "--channel", dummy_channel_1, "--list", "--with=dummy-a", "dummy-b"],
        stdout_contains="dummy-b",
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
