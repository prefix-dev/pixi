import os
import platform
import subprocess
from contextlib import contextmanager
from enum import IntEnum
from pathlib import Path
from typing import Generator

from rattler import Platform

PIXI_VERSION = "0.48.2"


ALL_PLATFORMS = '["linux-64", "osx-64", "osx-arm64", "win-64", "linux-ppc64le", "linux-aarch64"]'

CURRENT_PLATFORM = str(Platform.current())

EMPTY_BOILERPLATE_PROJECT = f"""
[workspace]
name = "test"
channels = []
platforms = ["{CURRENT_PLATFORM}"]
"""


class ExitCode(IntEnum):
    SUCCESS = 0
    FAILURE = 1
    INCORRECT_USAGE = 2
    COMMAND_NOT_FOUND = 127


class Output:
    command: list[Path | str]
    stdout: str
    stderr: str
    returncode: int

    def __init__(self, command: list[Path | str], stdout: str, stderr: str, returncode: int):
        self.command = command
        self.stdout = stdout
        self.stderr = stderr
        self.returncode = returncode

    def __str__(self) -> str:
        return f"command: {self.command}"


def verify_cli_command(
    command: list[Path | str],
    expected_exit_code: ExitCode = ExitCode.SUCCESS,
    stdout_contains: str | list[str] | None = None,
    stdout_excludes: str | list[str] | None = None,
    stderr_contains: str | list[str] | None = None,
    stderr_excludes: str | list[str] | None = None,
    env: dict[str, str] | None = None,
    cwd: str | Path | None = None,
    reset_env: bool = False,
) -> Output:
    base_env = {} if reset_env else dict(os.environ)
    complete_env = base_env if env is None else base_env | env
    # Set `NO_GRAPHICS` to avoid to have miette splitting up lines
    complete_env |= {"NO_GRAPHICS": "1"}

    process = subprocess.run(
        command,
        capture_output=True,
        env=complete_env,
        cwd=cwd,
    )
    # Decode stdout and stderr explicitly using UTF-8
    stdout = process.stdout.decode("utf-8", errors="replace")
    stderr = process.stderr.decode("utf-8", errors="replace")
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
            assert substring in stdout, f"'{substring}'\n not found in stdout:\n {stdout}"

    if stdout_excludes:
        if isinstance(stdout_excludes, str):
            stdout_excludes = [stdout_excludes]
        for substring in stdout_excludes:
            assert substring not in stdout, (
                f"'{substring}'\n unexpectedly found in stdout:\n {stdout}"
            )

    if stderr_contains:
        if isinstance(stderr_contains, str):
            stderr_contains = [stderr_contains]
        for substring in stderr_contains:
            assert substring in stderr, f"'{substring}'\n not found in stderr:\n {stderr}"

    if stderr_excludes:
        if isinstance(stderr_excludes, str):
            stderr_excludes = [stderr_excludes]
        for substring in stderr_excludes:
            assert substring not in stderr, (
                f"'{substring}'\n unexpectedly found in stderr:\n {stderr}"
            )

    return output


def bat_extension(exe_name: str) -> str:
    if platform.system() == "Windows":
        return exe_name + ".bat"
    else:
        return exe_name


def exec_extension(exe_name: str) -> str:
    if platform.system() == "Windows":
        return exe_name + ".exe"
    else:
        return exe_name


def is_binary(path: Path) -> bool:
    textchars = bytearray({7, 8, 9, 10, 12, 13, 27} | set(range(0x20, 0x100)) - {0x7F})
    with open(path, "rb") as f:
        return bool(f.read(2048).translate(None, bytes(textchars)))


def pixi_dir(project_root: Path) -> Path:
    return project_root.joinpath(".pixi")


def default_env_path(project_root: Path) -> Path:
    return pixi_dir(project_root).joinpath("envs", "default")


def repo_root() -> Path:
    return Path(__file__).parents[2]


def current_platform() -> str:
    return str(Platform.current())


def get_manifest(directory: Path) -> Path:
    pixi_toml = directory / "pixi.toml"
    pyproject_toml = directory / "pyproject.toml"

    if pixi_toml.exists():
        return pixi_toml
    elif pyproject_toml.exists():
        return pyproject_toml
    else:
        raise ValueError("Neither pixi.toml nor pyproject.toml found")


@contextmanager
def cwd(path: str | Path) -> Generator[None, None, None]:
    oldpwd = os.getcwd()
    os.chdir(path)
    try:
        yield
    finally:
        os.chdir(oldpwd)
