from pathlib import Path
import json
import platform

from .common import ALL_PLATFORMS, ExitCode, verify_cli_command


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


def test_shell_activation_order(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Validate that pixi shell env evaluation order matches the new rules.

    Expectations:
      - Pre-activation metadata env vars are visible to activation scripts.
      - Post-activation env (activation.env) overrides values set by scripts.
    """
    is_windows = platform.system() == "Windows"

    # Create activation script that sets an override and uses metadata vars
    scripts_dir = tmp_pixi_workspace.joinpath("scripts")
    scripts_dir.mkdir(parents=True, exist_ok=True)
    if is_windows:
        script_name = "activate.bat"
        script_content = (
            "@echo off\n"
            "set VAR_OVERRIDE=from_script\n"
            "set FROM_SCRIPT=%PIXI_PROJECT_NAME%:%CONDA_DEFAULT_ENV%\n"
        )
        target_activation = "[feature.f.target.win-64.activation]"
    else:
        script_name = "activate.sh"
        script_content = (
            "#!/usr/bin/env bash\n"
            "export VAR_OVERRIDE=from_script\n"
            'export FROM_SCRIPT="$PIXI_PROJECT_NAME:$CONDA_DEFAULT_ENV"\n'
        )
        target_activation = "[feature.f.target.unix.activation]"

    (scripts_dir / script_name).write_text(script_content)

    # Minimal manifest: add post-activation env override and point to the script
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = f"""
    [workspace]
    name = "order-test"
    channels = []
    platforms = {ALL_PLATFORMS}

    [feature.f.activation]
    env.VAR_OVERRIDE = "from_activation_env"

    {target_activation}
    scripts = ["scripts/{script_name}"]

    [environments]
    default = ["f"]
    """
    manifest.write_text(toml)

    # Ask pixi to compute the environment for shell activation via shell-hook --json
    out = verify_cli_command(
        [pixi, "shell-hook", "--manifest-path", manifest, "--json"],
        expected_exit_code=ExitCode.SUCCESS,
    )

    data = json.loads(out.stdout)
    env = data["environment_variables"]

    # activation.env overrides activation script value
    assert env.get("VAR_OVERRIDE") == "from_activation_env"
    # activation script can see pre-activation metadata variables
    assert env.get("FROM_SCRIPT") == "order-test:order-test"
