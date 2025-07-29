from pathlib import Path
import platform

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
    )

    # Test PowerShell completions (available on all platforms)
    # PowerShell completions are handled via PowerShell profile, not shell hook
    verify_cli_command(
        [pixi, "shell-hook", "--manifest-path", tmp_pixi_workspace, "--shell", "powershell"],
        stdout_excludes=[
            "Scripts/_pixi.ps1"
        ],  # PowerShell doesn't source completions in shell hook
    )

    # Windows-specific shells
    if platform.system() == "Windows":
        # Test cmd.exe completions (Windows-only)
        cmd_comp_dir = ".pixi/envs/default/Scripts"
        tmp_pixi_workspace.joinpath(cmd_comp_dir).mkdir(parents=True, exist_ok=True)
        tmp_pixi_workspace.joinpath(cmd_comp_dir, "pixi.cmd").touch()

        verify_cli_command(
            [pixi, "shell-hook", "--manifest-path", tmp_pixi_workspace, "--shell", "cmd"],
            stdout_contains=['@SET "Path=', "Scripts", "@PROMPT"],
        )
    else:
        # Bash completions
        bash_comp_dir = ".pixi/envs/default/share/bash-completion/completions"
        tmp_pixi_workspace.joinpath(bash_comp_dir).mkdir(parents=True, exist_ok=True)
        tmp_pixi_workspace.joinpath(bash_comp_dir, "pixi.sh").touch()

        verify_cli_command(
            [pixi, "shell-hook", "--manifest-path", tmp_pixi_workspace, "--shell", "bash"],
            stdout_contains=["source", "share/bash-completion/completions"],
        )

        # Zsh completions
        zsh_comp_dir = ".pixi/envs/default/share/zsh/site-functions"
        tmp_pixi_workspace.joinpath(zsh_comp_dir).mkdir(parents=True, exist_ok=True)
        tmp_pixi_workspace.joinpath(zsh_comp_dir, "_pixi").touch()

        verify_cli_command(
            [pixi, "shell-hook", "--manifest-path", tmp_pixi_workspace, "--shell", "zsh"],
            stdout_contains=["fpath+=", "share/zsh/site-functions", "autoload -Uz compinit"],
        )

        # Fish completions
        fish_comp_dir = ".pixi/envs/default/share/fish/vendor_completions.d"
        tmp_pixi_workspace.joinpath(fish_comp_dir).mkdir(parents=True, exist_ok=True)
        tmp_pixi_workspace.joinpath(fish_comp_dir, "pixi.fish").touch()

        verify_cli_command(
            [pixi, "shell-hook", "--manifest-path", tmp_pixi_workspace, "--shell", "fish"],
            stdout_contains=["for file in", "source", "share/fish/vendor_completions.d"],
        )
