from enum import IntEnum
from pathlib import Path
import subprocess

PIXI_VERSION = "0.32.1"


class ExitCode(IntEnum):
    SUCCESS = 0
    FAILURE = 1
    INCORRECT_USAGE = 2


def verify_cli_command(
    command: list[Path | str],
    expected_exit_code: ExitCode,
    stdout_contains: str | list[str] | None = None,
    stdout_excludes: str | list[str] | None = None,
    stderr_contains: str | list[str] | None = None,
    stderr_excludes: str | list[str] | None = None,
) -> None:
    process = subprocess.run(command, capture_output=True, text=True)
    stdout, stderr, returncode = process.stdout, process.stderr, process.returncode
    print(f"command: {command}, stdout: {stdout}, stderr: {stderr}, code: {returncode}")
    if expected_exit_code is not None:
        assert (
            returncode == expected_exit_code
        ), f"Return code was {returncode}, expected {expected_exit_code}, stderr: {stderr}"

    if stdout_contains:
        if isinstance(stdout_contains, str):
            stdout_contains = [stdout_contains]
        for substring in stdout_contains:
            assert substring in stdout, f"'{substring}' not found in stdout: {stdout}"

    if stdout_excludes:
        if isinstance(stdout_excludes, str):
            stdout_excludes = [stdout_excludes]
        for substring in stdout_excludes:
            assert substring not in stdout, f"'{substring}' unexpectedly found in stdout: {stdout}"

    if stderr_contains:
        if isinstance(stderr_contains, str):
            stderr_contains = [stderr_contains]
        for substring in stderr_contains:
            assert substring in stderr, f"'{substring}' not found in stderr: {stderr}"

    if stderr_excludes:
        if isinstance(stderr_excludes, str):
            stderr_excludes = [stderr_excludes]
        for substring in stderr_excludes:
            assert substring not in stderr, f"'{substring}' unexpectedly found in stderr: {stderr}"
