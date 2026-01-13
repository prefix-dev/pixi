"""Common utilities for pixi-build-ros integration tests."""

import os
import platform
import re
import shutil
import subprocess
from collections.abc import Sequence
from enum import IntEnum
from pathlib import Path
from typing import Any


# Regex pattern to match ANSI escape sequences
ANSI_ESCAPE_PATTERN = re.compile(r"\x1b\[[0-9;]*m")


class ExitCode(IntEnum):
    SUCCESS = 0
    FAILURE = 1
    INCORRECT_USAGE = 2
    COMMAND_NOT_FOUND = 127


class Output:
    command: Sequence[Path | str]
    stdout: str
    stderr: str
    returncode: int

    def __init__(self, command: Sequence[Path | str], stdout: str, stderr: str, returncode: int):
        self.command = command
        self.stdout = stdout
        self.stderr = stderr
        self.returncode = returncode

    def __str__(self) -> str:
        return f"command: {self.command}"


def verify_cli_command(
    command: Sequence[Path | str],
    expected_exit_code: ExitCode = ExitCode.SUCCESS,
    stdout_contains: str | list[str] | None = None,
    stdout_excludes: str | list[str] | None = None,
    stderr_contains: str | list[str] | None = None,
    stderr_excludes: str | list[str] | None = None,
    env: dict[str, str] | None = None,
    cwd: str | Path | None = None,
    reset_env: bool = False,
    strip_ansi: bool = False,
) -> Output:
    base_env = {} if reset_env else dict(os.environ)
    # Remove all PIXI_ prefixed env vars to avoid interference from the outer environment
    base_env = {k: v for k, v in base_env.items() if not k.startswith("PIXI_")}
    complete_env = base_env if env is None else base_env | env
    # Set `PIXI_NO_WRAP` to avoid to have miette wrapping lines
    complete_env |= {"PIXI_NO_WRAP": "1"}

    process = subprocess.run(
        command,
        capture_output=True,
        env=complete_env,
        cwd=cwd,
    )
    # Decode stdout and stderr explicitly using UTF-8
    stdout = process.stdout.decode("utf-8", errors="replace")
    stderr = process.stderr.decode("utf-8", errors="replace")

    # Optionally strip ANSI escape sequences for matching
    stdout_for_matching = ANSI_ESCAPE_PATTERN.sub("", stdout) if strip_ansi else stdout
    stderr_for_matching = ANSI_ESCAPE_PATTERN.sub("", stderr) if strip_ansi else stderr

    returncode = process.returncode
    output = Output(command, stdout, stderr, returncode)
    print(f"command: {command}, stdout: {stdout}, stderr: {stderr}, code: {returncode}")
    assert returncode == expected_exit_code, (
        f"Return code was {returncode}, expected {expected_exit_code}, stderr: {stderr}"
    )

    if stdout_contains:
        if isinstance(stdout_contains, str):
            stdout_contains = [stdout_contains]
        for substring in stdout_contains:
            assert substring in stdout_for_matching, (
                f"'{substring}'\n not found in stdout:\n {stdout}"
            )

    if stdout_excludes:
        if isinstance(stdout_excludes, str):
            stdout_excludes = [stdout_excludes]
        for substring in stdout_excludes:
            assert substring not in stdout_for_matching, (
                f"'{substring}'\n unexpectedly found in stdout:\n {stdout}"
            )

    if stderr_contains:
        if isinstance(stderr_contains, str):
            stderr_contains = [stderr_contains]
        for substring in stderr_contains:
            assert substring in stderr_for_matching, (
                f"'{substring}'\n not found in stderr:\n {stderr}"
            )

    if stderr_excludes:
        if isinstance(stderr_excludes, str):
            stderr_excludes = [stderr_excludes]
        for substring in stderr_excludes:
            assert substring not in stderr_for_matching, (
                f"'{substring}'\n unexpectedly found in stderr:\n {stderr}"
            )

    return output


def exec_extension(exe_name: str) -> str:
    if platform.system() == "Windows":
        return exe_name + ".exe"
    return exe_name


def copy_manifest(
    src: os.PathLike[str],
    dst: os.PathLike[str],
) -> Path:
    """Copy file (simple copy)."""
    return Path(shutil.copy(src, dst))


def copytree_with_local_backend(
    src: os.PathLike[str],
    dst: os.PathLike[str],
    **kwargs: Any,
) -> Path:
    """Copy tree while ignoring .pixi directories and .conda files."""
    kwargs.setdefault("copy_function", copy_manifest)

    return Path(
        shutil.copytree(src, dst, ignore=shutil.ignore_patterns(".pixi", "*.conda"), **kwargs)
    )
