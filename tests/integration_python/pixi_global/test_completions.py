from pathlib import Path
import platform

from ..common import exec_extension, verify_cli_command


def bash_completions(pixi_home: Path, executable: str) -> Path:
    return pixi_home.joinpath("completions", "bash", executable)


def zsh_completions(pixi_home: Path, executable: str) -> Path:
    return pixi_home.joinpath("completions", "zsh", f"_{executable}")


def fish_completions(pixi_home: Path, executable: str) -> Path:
    return pixi_home.joinpath("completions", "fish", f"{executable}.fish")


def test_sync_exposes_completions(
    pixi: Path, tmp_pixi_workspace: Path, completions_channel_1: str
) -> None:
    env = {"PIXI_HOME": str(tmp_pixi_workspace)}
    manifests = tmp_pixi_workspace.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    toml = f"""
    [envs.test]
    channels = ["{completions_channel_1}"]
    dependencies = {{ ripgrep-completions = "*" }}
    exposed = {{ rg = "rg" }}
    """
    manifest.write_text(toml)
    rg = tmp_pixi_workspace / "bin" / exec_extension("rg")

    # Completions
    bash = bash_completions(tmp_pixi_workspace, "rg")
    zsh = zsh_completions(tmp_pixi_workspace, "rg")
    fish = fish_completions(tmp_pixi_workspace, "rg")

    # Test basic commands
    verify_cli_command([pixi, "global", "sync"], env=env)
    assert rg.is_file()

    if platform.system() == "Windows":
        # Completions are ignored on Windows
        assert not bash.is_file()
        assert not zsh.is_file()
        assert not fish.is_file()
    else:
        assert bash.is_file()
        assert zsh.is_file()
        assert fish.is_file()

    # If the exposed executable is removed, the same should happen for the completions
    verify_cli_command([pixi, "global", "expose", "remove", "rg"], env=env)
    assert not bash.is_file()
    assert not zsh.is_file()
    assert not fish.is_file()


def test_only_self_expose_have_completions(
    pixi: Path, tmp_pixi_workspace: Path, completions_channel_1: str
) -> None:
    env = {"PIXI_HOME": str(tmp_pixi_workspace)}

    # Install `ripgrep-completions`, but expose `rg` under `ripgrep`
    # Therefore no completions should be installed
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            completions_channel_1,
            "--expose",
            "ripgrep=rg",
            "ripgrep-completions",
        ],
        env=env,
    )

    # Completions
    bash = bash_completions(tmp_pixi_workspace, "rg")
    zsh = zsh_completions(tmp_pixi_workspace, "rg")
    fish = fish_completions(tmp_pixi_workspace, "rg")

    assert not bash.is_file()
    assert not zsh.is_file()
    assert not fish.is_file()

    # When we add `rg=rg`, the completions should be installed
    verify_cli_command(
        [pixi, "global", "expose", "add", "--environment", "ripgrep-completions", "rg=rg"], env=env
    )

    if platform.system() == "Windows":
        # Completions are ignored on Windows
        assert not bash.is_file()
        assert not zsh.is_file()
        assert not fish.is_file()
    else:
        assert bash.is_file()
        assert zsh.is_file()
        assert fish.is_file()

    # By uninstalling the environment, the completions should be removed as well
    verify_cli_command([pixi, "global", "uninstall", "ripgrep-completions"], env=env)

    assert not bash.is_file()
    assert not zsh.is_file()
    assert not fish.is_file()
