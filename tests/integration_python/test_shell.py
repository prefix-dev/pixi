from pathlib import Path

from .common import ExitCode, verify_cli_command


def test_shell_hook_completions(
    pixi: Path, tmp_pixi_workspace: Path, completions_channel_1: str
) -> None:
    # Create a new workspace
    verify_cli_command(
        [pixi, "init", "--channel", completions_channel_1, tmp_pixi_workspace], ExitCode.SUCCESS
    )

    verify_cli_command(
        [pixi, "add", "--manifest-path", tmp_pixi_workspace, "ripgrep-completions"],
        ExitCode.SUCCESS,
    )

    # Completions are sourced by default
    verify_cli_command(
        [pixi, "shell-hook", "--manifest-path", tmp_pixi_workspace, "--shell", "bash"],
        ExitCode.SUCCESS,
        stdout_contains="bash-completion",
    )

    # Opt-out of sourcing completions
    verify_cli_command(
        [
            pixi,
            "shell-hook",
            "--manifest-path",
            tmp_pixi_workspace,
            "--shell",
            "bash",
            "--no-completions",
        ],
        ExitCode.SUCCESS,
        stdout_excludes="bash-completion",
    )
