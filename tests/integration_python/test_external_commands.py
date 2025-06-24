import os
import platform
import stat
from pathlib import Path


from .common import ExitCode, bat_extension, verify_cli_command


def create_external_command(command_path: Path, script_content: str) -> Path:
    """Helper function to create a mock external pixi command"""
    command_path.write_text(script_content)

    # Make executable on Unix systems
    if platform.system() != "Windows":
        current_mode = command_path.stat().st_mode
        command_path.chmod(current_mode | stat.S_IEXEC)

    return command_path


def test_external_command_execution(
    pixi: Path, tmp_pixi_workspace: Path, external_commands_dir: Path
) -> None:
    """Test that external pixi commands can be discovered and executed"""

    # Create a simple external command
    if platform.system() == "Windows":
        script_content = "@echo off\necho Hello from pixi-test extension!\necho Args: %*"
    else:
        script_content = "#!/bin/bash\necho 'Hello from pixi-test extension!'\necho \"Args: $@\""

    # Create pixi-test command
    external_cmd = external_commands_dir / bat_extension("pixi-test")
    create_external_command(external_cmd, script_content)

    # Add external commands directory to PATH
    env = {"PATH": f"{external_commands_dir}{os.pathsep}{os.environ.get('PATH', '')}"}

    # Test external command execution
    verify_cli_command(
        [pixi, "test", "arg1", "arg2"],
        env=env,
        cwd=tmp_pixi_workspace,
        stdout_contains=[
            "Hello from pixi-test extension!",
            "Args: arg1 arg2",
        ],
    )


def test_external_command_not_found(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Test that non-existent external commands provide error messages"""

    # Test unknown command
    verify_cli_command(
        [pixi, "nonexistent"],
        ExitCode.FAILURE,
        cwd=tmp_pixi_workspace,
        stderr_contains="No such command: `pixi nonexistent`",
    )
