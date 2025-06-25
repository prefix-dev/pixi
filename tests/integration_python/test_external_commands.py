import platform
import stat
from pathlib import Path


from .common import ExitCode, verify_cli_command


def create_external_command(command_path: Path, script_content: str) -> Path:
    """Helper function to create a mock external pixi command"""
    command_path.write_text(script_content)

    # Make executable on Unix systems
    if platform.system() != "Windows":
        current_mode = command_path.stat().st_mode
        command_path.chmod(current_mode | stat.S_IEXEC)

    return command_path


def test_external_extension(pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_pixi_workspace)}

    pixi_foobar = tmp_pixi_workspace / "bin"

    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "pixi-foobar",
        ],
        env=env,
    )

    # Add external commands directory to PATH
    env = {"PATH": str(pixi_foobar)}

    # Test external command execution
    verify_cli_command(
        [pixi, "foobar", "arg1", "arg2"],
        env=env,
        cwd=tmp_pixi_workspace,
        stdout_contains=[
            "arg1 arg2",
        ],
    )


def test_external_command_not_found(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    """Test that non-existent external commands provide error messages"""
    env = {"PIXI_HOME": str(tmp_pixi_workspace)}

    pixi_foobar = tmp_pixi_workspace / "bin"

    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "pixi-foobar",
        ],
        env=env,
    )

    # Add external commands directory to PATH
    env = {"PATH": str(pixi_foobar)}

    # Test external command execution
    verify_cli_command(
        [pixi, "nonexistent"],
        env=env,
        cwd=tmp_pixi_workspace,
        stderr_contains="No such command: `pixi nonexistent`",
        expected_exit_code=ExitCode.INCORRECT_USAGE,
    )


def test_pixi_internal_wins_over_external(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    env = {"PIXI_HOME": str(tmp_pixi_workspace)}

    pixi_foobar = tmp_pixi_workspace / "bin"

    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--expose",
            "pixi-list=pixi-foobar",
            "--channel",
            dummy_channel_1,
            "pixi-foobar",
        ],
        env=env,
    )

    # Add external commands directory to PATH
    env = {"PATH": str(pixi_foobar)}

    # We want to make sure that pixi list is executed instead of the
    # external command ( pixi-foobar that we exposed as pixi-list )
    verify_cli_command(
        [pixi, "list"],
        env=env,
        cwd=tmp_pixi_workspace,
        stdout_contains=[
            "Kind",
            "Build",
        ],
    )
