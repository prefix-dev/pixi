from pathlib import Path
import platform

from ..common import exec_extension, verify_cli_command


def bash_completions(prefix: Path, executable: str) -> Path:
    return prefix.joinpath("share", "bash-completions", executable)


def zsh_completions(prefix: Path, executable: str) -> Path:
    return prefix.joinpath("zsh", "site-functions", f"_{executable}")


def fish_completions(prefix: Path, executable: str) -> Path:
    return prefix.joinpath("fish", "vendor_completions.d", f"{executable}.fish")


def test_sync_change_expose(
    pixi: Path, tmp_pixi_workspace: Path, completions_channel_1: str
) -> None:
    env = {"PIXI_HOME": str(tmp_pixi_workspace)}
    manifests = tmp_pixi_workspace.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    toml = f"""
    [envs.test]
    channels = ["{completions_channel_1}"]
    [envs.test]
    dependencies = {{ ripgrep-completions = "*" }}
    exposed = {{ rg = rg }}
    """
    manifest.write_text(toml)
    rg = tmp_pixi_workspace / "bin" / exec_extension("rg")
    prefix = tmp_pixi_workspace.joinpath(".pixi", "envs", "default")

    # Test basic commands
    verify_cli_command([pixi, "global", "sync"], env=env)
    assert rg.is_file()

    bash = bash_completions(prefix, "rg")
    zsh = zsh_completions(prefix, "rg")
    fish = fish_completions(prefix, "rg")

    if platform.system() == "Windows":
        bash.is_file()
        zsh.is_file()
        fish.is_file()
    else:
        not bash.is_file()
        not zsh.is_file()
        not fish.is_file()
