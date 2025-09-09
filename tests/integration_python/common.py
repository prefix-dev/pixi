import os
import platform
import subprocess
from contextlib import contextmanager
from enum import IntEnum
from pathlib import Path
import sys
from typing import Generator, Optional, Sequence, Set, Tuple

from rattler import Platform

PIXI_VERSION = "0.54.2"


ALL_PLATFORMS = '["linux-64", "osx-64", "osx-arm64", "win-64", "linux-ppc64le", "linux-aarch64"]'

CURRENT_PLATFORM = str(Platform.current())

EMPTY_BOILERPLATE_PROJECT = f"""
[workspace]
name = "test"
channels = []
platforms = ["{CURRENT_PLATFORM}"]
"""

CONDA_FORGE_CHANNEL = "https://prefix.dev/conda-forge"


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
) -> Output:
    base_env = {} if reset_env else dict(os.environ)
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


def run_and_get_env(pixi: Path, *args: str, env_var: str) -> Tuple[Optional[str], Output]:
    if sys.platform.startswith("win"):
        cmd = [str(pixi), "exec", *args, "--", "cmd", "/c", f"echo %{env_var}%"]
    else:
        cmd = [str(pixi), "exec", *args, "--", "printenv", env_var]

    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
        )

        value = result.stdout.strip()

        output = Output(
            command=cmd,
            stdout=value,
            stderr=result.stderr.strip(),
            returncode=result.returncode,
        )

        return (value if value and value != f"%{env_var}%" else None, output)
    except Exception as e:
        print(f"Error running command: {e}")
        print(f"Command: {' '.join(cmd)}")
        raise


# Command discovery utilities for testing CLI flag support


def discover_pixi_commands() -> set[str]:
    """Discover all available pixi commands by walking the docs/reference/cli/pixi directory.

    Returns:
        Set[str]: Set of command names in the format "pixi command subcommand ..."

    Examples:
        {"pixi add", "pixi workspace channel add", "pixi shell", ...}
    """
    docs_path = repo_root() / "docs" / "reference" / "cli" / "pixi"
    commands: set[str] = set()

    if not docs_path.exists():
        return commands

    # Walk through all markdown files in the docs directory
    for md_file in docs_path.rglob("*.md"):
        # Get relative path from the pixi docs directory
        relative_path = md_file.relative_to(docs_path)

        # Convert file path to command format
        # e.g., "workspace/channel/add.md" -> "pixi workspace channel add"
        command_parts = ["pixi"] + list(relative_path.parts[:-1]) + [relative_path.stem]
        command = " ".join(command_parts)
        commands.add(command)

    return commands


def check_command_supports_flags(command_parts: list[str], *flag_names: str) -> tuple[bool, ...]:
    """Check if a command supports specific flags by examining its documentation.

    Args:
        command_parts: List of command parts (e.g., ["workspace", "channel", "add"])
        *flag_names: Variable number of flag names to check for (e.g., "--frozen", "--no-install")

    Returns:
        Tuple[bool, ...]: Tuple of booleans indicating support for each flag in order

    Examples:
        check_command_supports_flags(["add"], "--frozen", "--no-install")
        # Returns: (True, True) if both flags are supported

        check_command_supports_flags(["shell"], "--frozen", "--locked", "--no-install")
        # Returns: (False, True, True) if only --locked and --no-install are supported
    """
    # Build the documentation file path
    docs_path = repo_root() / "docs" / "reference" / "cli" / "pixi"
    doc_file = docs_path / Path(*command_parts).with_suffix(".md")

    if not doc_file.exists():
        return tuple(False for _ in flag_names)

    try:
        doc_content = doc_file.read_text()

        # Check each flag
        results = []
        for flag_name in flag_names:
            results.append(flag_name in doc_content)

        return tuple(results)

    except (OSError, IOError):
        return tuple(False for _ in flag_names)


def find_commands_supporting_flags(*flag_names: str) -> list[str]:
    """Find all pixi commands that support ALL of the specified flags.

    Args:
        *flag_names: Variable number of flag names that commands must support

    Returns:
        List[str]: List of command names that support all specified flags

    Examples:
        find_commands_supporting_flags("--frozen", "--no-install")
        # Returns: ["pixi add", "pixi remove", "pixi run", ...]

        find_commands_supporting_flags("--locked", "--no-install")
        # Returns: ["pixi shell"] (special case that uses --locked instead of --frozen)
    """
    all_commands = discover_pixi_commands()
    supported_commands = []

    for command_str in all_commands:
        # Skip the "pixi" prefix to get command parts
        command_parts = (
            command_str.split()[1:] if command_str.startswith("pixi ") else command_str.split()
        )

        # Skip empty commands
        if not command_parts:
            continue

        # Check if the command supports all specified flags
        flag_support = check_command_supports_flags(command_parts, *flag_names)

        # Only include if ALL flags are supported
        if all(flag_support):
            supported_commands.append(command_str)

    return sorted(supported_commands)


def find_commands_supporting_frozen_and_no_install() -> Set[str]:
    """Convenience function to find commands supporting both --frozen and --no-install flags.

    This also includes commands that use --locked instead of --frozen (like pixi shell).

    Returns:
        List[str]: List of command names supporting freeze/lock and no-install functionality
    """
    # Find commands that support --frozen and --no-install
    return set(find_commands_supporting_flags("--frozen", "--no-install"))
