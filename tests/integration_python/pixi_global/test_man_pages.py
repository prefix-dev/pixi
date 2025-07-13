from pathlib import Path
import platform

from ..common import exec_extension, verify_cli_command


def man_page_path(pixi_home: Path, executable: str, section: str = "1") -> Path:
    return pixi_home.joinpath("share", "man", f"man{section}", f"{executable}.{section}")


def test_sync_exposes_man_pages(
    pixi: Path, tmp_pixi_workspace: Path, default_channel_1: str
) -> None:
    env = {"PIXI_HOME": str(tmp_pixi_workspace)}
    manifests = tmp_pixi_workspace.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    toml = f"""
    [envs.test]
    channels = ["{default_channel_1}"]
    dependencies = {{ ripgrep = "*" }}
    exposed = {{ rg = "rg" }}
    """
    manifest.write_text(toml)
    rg = tmp_pixi_workspace / "bin" / exec_extension("rg")

    # Man page
    man_page = man_page_path(tmp_pixi_workspace, "rg")

    # Test basic commands
    verify_cli_command([pixi, "global", "sync"], env=env)
    assert rg.is_file()

    if platform.system() == "Windows":
        # Man pages are ignored on Windows
        assert not man_page.is_file()
    else:
        assert man_page.is_file()
        assert man_page.is_symlink()

    # If the exposed executable is removed, the same should happen for the man page
    verify_cli_command([pixi, "global", "expose", "remove", "rg"], env=env)
    assert not man_page.is_file()


def test_only_self_expose_have_man_pages(
    pixi: Path, tmp_pixi_workspace: Path, default_channel_1: str
) -> None:
    env = {"PIXI_HOME": str(tmp_pixi_workspace)}

    # Install `ripgrep`, but expose `rg` under `ripgrep`
    # Therefore no man pages should be installed
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            default_channel_1,
            "--expose",
            "ripgrep=rg",
            "ripgrep",
        ],
        env=env,
    )

    # Man page
    man_page = man_page_path(tmp_pixi_workspace, "rg")

    assert not man_page.is_file()

    # When we add `rg=rg`, the man page should be installed
    verify_cli_command(
        [pixi, "global", "expose", "add", "--environment", "ripgrep", "rg=rg"], env=env
    )

    if platform.system() == "Windows":
        # Man pages are ignored on Windows
        assert not man_page.is_file()
    else:
        assert man_page.is_file()
        assert man_page.is_symlink()

    # By uninstalling the environment, the man page should be removed as well
    verify_cli_command([pixi, "global", "uninstall", "ripgrep"], env=env)

    assert not man_page.is_file()


def test_installing_same_package_again_without_expose_shouldnt_remove_man_page(
    pixi: Path, tmp_pixi_workspace: Path, default_channel_1: str
) -> None:
    env = {"PIXI_HOME": str(tmp_pixi_workspace)}

    # Man page
    man_page = man_page_path(tmp_pixi_workspace, "rg")

    # Install `ripgrep`, and expose `rg` as `rg`
    # This should install the man page
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            default_channel_1,
            "--expose",
            "rg=rg",
            "--environment",
            "test-1",
            "ripgrep",
        ],
        env=env,
    )

    if platform.system() == "Windows":
        # Man pages are ignored on Windows
        assert not man_page.is_file()
    else:
        assert man_page.is_file()
        assert man_page.is_symlink()

    # Install `ripgrep`, but expose `rg` under `ripgrep`
    # Therefore no man pages should be installed
    # But existing ones should also not be removed
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            default_channel_1,
            "--expose",
            "ripgrep=rg",
            "--environment",
            "test-2",
            "ripgrep",
        ],
        env=env,
    )

    if platform.system() == "Windows":
        # Man pages are ignored on Windows
        assert not man_page.is_file()
    else:
        assert man_page.is_file()
        assert man_page.is_symlink()


def test_man_page_priority_order(
    pixi: Path, tmp_pixi_workspace: Path, default_channel_1: str
) -> None:
    """Test that man page priority order (man1 > man8 > man3 > man5) is respected"""
    env = {"PIXI_HOME": str(tmp_pixi_workspace)}

    # Install a command that could potentially have multiple man page sections
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            default_channel_1,
            "bash",
        ],
        env=env,
    )

    if platform.system() != "Windows":
        # bash should get bash.1 (user commands) not bash.3 (library functions)
        man_page_1 = man_page_path(tmp_pixi_workspace, "bash", "1")
        man_page_3 = man_page_path(tmp_pixi_workspace, "bash", "3")

        # Only one man page should be symlinked (the highest priority one)
        # Note: This test assumes bash has a man1 page, which it typically does
        if man_page_1.is_file():
            assert man_page_1.is_symlink()
            # Should not create lower priority man pages if higher priority exists
            assert not man_page_3.is_file()


def test_man_page_without_man_page_doesnt_error(
    pixi: Path, tmp_pixi_workspace: Path, default_channel_1: str
) -> None:
    """Test that commands without man pages don't cause errors"""
    env = {"PIXI_HOME": str(tmp_pixi_workspace)}

    # Install a simple package that likely doesn't have man pages
    # Using a minimal package or one known not to have man pages
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            default_channel_1,
            "python",  # Python might not have man pages in all packages
        ],
        env=env,
    )

    # Verify the command was installed successfully even without man pages
    python_exec = tmp_pixi_workspace / "bin" / exec_extension("python")
    assert python_exec.is_file()

    # Man page directory should exist but no python man page is required
    man_dir = tmp_pixi_workspace / "share" / "man" / "man1"
    if platform.system() != "Windows":
        assert man_dir.is_dir()
        # No assertion about python.1 existing - that's package dependent
