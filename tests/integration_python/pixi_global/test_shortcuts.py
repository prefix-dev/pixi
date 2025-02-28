from pathlib import Path
import tomllib
import tomli_w
from typing import List, TypedDict, Dict, Callable

from ..common import verify_cli_command, ExitCode, CURRENT_PLATFORM


class PlatformConfig(TypedDict):
    shortcut_path: Callable[[Path, str], Path]
    shortcut_exists: Callable[[Path], bool]


# Platform-specific configuration
PLATFORM_CONFIG: Dict[str, PlatformConfig] = {
    "linux-64": {
        "shortcut_path": lambda data_home, name: data_home
        / "applications"
        / f"{name}_{name}.desktop",
        "shortcut_exists": lambda path: path.is_file(),
    },
    "osx-arm64": {  # macOS
        "shortcut_path": lambda data_home, name: data_home / "Applications" / f"{name}.app",
        "shortcut_exists": lambda path: path.is_dir(),
    },
    "osx64": {  # macOS
        "shortcut_path": lambda data_home, name: data_home / "Applications" / f"{name}.app",
        "shortcut_exists": lambda path: path.is_dir(),
    },
    "win-64": {
        "shortcut_path": lambda data_home, name: data_home
        / "Microsoft"
        / "Windows"
        / "Start Menu"
        / "Programs"
        / f"{name}.lnk",
        "shortcut_exists": lambda path: path.is_file(),
    },
}


def verify_shortcuts_exist(
    data_home: Path,
    shortcut_names: List[str],
    expected_exists: bool,
) -> None:
    """Verify if the specified shortcuts exist or not on the given system."""
    # Using the key to get the platform-specific configuration, to force a KeyError if the key is not found
    system = CURRENT_PLATFORM
    config = PLATFORM_CONFIG[system]
    for name in shortcut_names:
        shortcut_path = config["shortcut_path"](data_home, name)
        exists = config["shortcut_exists"](shortcut_path)
        assert (
            exists == expected_exists
        ), f"Shortcut '{name}' {'should' if expected_exists else 'should not'} exist on {system}"


def test_sync_creation_and_removal(
    pixi: Path,
    tmp_path: Path,
    shortcuts_channel_1: str,
) -> None:
    """Test shortcut creation and removal with sync."""
    pixi_home = tmp_path / "pixi_home"
    data_home = tmp_path / "data_home"
    env = {"PIXI_HOME": str(pixi_home), "XDG_DATA_HOME": str(data_home), "HOME": str(data_home)}

    # Setup manifest with given shortcuts
    manifests = pixi_home.joinpath("manifests")
    manifests.mkdir(parents=True)
    manifest = manifests.joinpath("pixi-global.toml")
    toml = f"""
    [envs.test]
    channels = ["{shortcuts_channel_1}"]
    dependencies = {{ pixi-editor = "*" }}
    """
    # Verify no shortcuts exist after sync
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=env)
    verify_shortcuts_exist(data_home, ["pixi-editor"], expected_exists=False)

    parsed_toml = tomllib.loads(toml)
    parsed_toml["envs"]["test"]["shortcuts"] = ["pixi-editor"]
    manifest.write_text(tomli_w.dumps(parsed_toml))

    # # Run sync and verify
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=env)
    verify_shortcuts_exist(data_home, ["pixi-editor"], expected_exists=True)

    # test removal of shortcuts
    del parsed_toml["envs"]["test"]["shortcuts"]
    manifest.write_text(tomli_w.dumps(parsed_toml))
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=env)
    verify_shortcuts_exist(data_home, ["pixi-editor"], expected_exists=False)


# TODO: test empty list of shortcuts
# TODO: test requesting shortcuts that are not available
# TODO: test that shortcuts are removed when environment is removed


def test_install_simple(
    pixi: Path,
    tmp_path: Path,
    shortcuts_channel_1: str,
) -> None:
    """Test shortcut creation with install."""
    pixi_home = tmp_path / "pixi_home"
    data_home = tmp_path / "data_home"
    env = {"PIXI_HOME": str(pixi_home), "XDG_DATA_HOME": str(data_home), "HOME": str(data_home)}

    # Verify no shortcuts exist after sync
    verify_cli_command(
        [pixi, "global", "install", "--channel", shortcuts_channel_1, "pixi-editor"],
        ExitCode.SUCCESS,
        env=env,
    )

    # Verify manifest
    manifest = pixi_home.joinpath("manifests", "pixi-global.toml")
    parsed_toml = tomllib.loads(manifest.read_text())
    assert parsed_toml["envs"]["pixi-editor"]["shortcuts"] == ["pixi-editor"]

    # Verify shortcut exists
    verify_shortcuts_exist(data_home, ["pixi-editor"], expected_exists=True)


def test_install_no_shortcut(
    pixi: Path,
    tmp_path: Path,
    shortcuts_channel_1: str,
) -> None:
    """Test shortcut creation with install where `--no-shortcut` was passed."""
    pixi_home = tmp_path / "pixi_home"
    data_home = tmp_path / "data_home"
    env = {"PIXI_HOME": str(pixi_home), "XDG_DATA_HOME": str(data_home), "HOME": str(data_home)}

    # Verify no shortcuts exist after sync
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            shortcuts_channel_1,
            "--no-shortcut",
            "pixi-editor",
        ],
        ExitCode.SUCCESS,
        env=env,
    )

    # Verify manifest
    manifest = pixi_home.joinpath("manifests", "pixi-global.toml")
    parsed_toml = tomllib.loads(manifest.read_text())
    assert "shortcuts" not in parsed_toml["envs"]["pixi-editor"]

    # Verify shortcut does not exist
    verify_shortcuts_exist(data_home, ["pixi-editor"], expected_exists=False)
