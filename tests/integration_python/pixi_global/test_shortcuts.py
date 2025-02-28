from pathlib import Path
import tomllib
import tomli_w
from ..common import verify_cli_command, ExitCode, CURRENT_PLATFORM
from abc import ABC, abstractmethod
import pytest
from dataclasses import dataclass


@dataclass
class SetupData:
    pixi_home: Path
    data_home: Path
    env: dict[str, str]


@pytest.fixture
def setup_data(tmp_path: Path) -> SetupData:
    pixi_home = tmp_path / "pixi_home"
    data_home = tmp_path / "data_home"
    env = {
        "PIXI_HOME": str(pixi_home),
        "HOME": str(data_home),  # Used for macOS and Linux
        "MENUINST_FAKE_DIRECTORIES": str(data_home),  # Used for Windows
    }
    return SetupData(pixi_home=pixi_home, data_home=data_home, env=env)


class PlatformConfig(ABC):
    @abstractmethod
    def shortcut_path(self, data_home: Path, name: str) -> Path:
        pass

    @abstractmethod
    def shortcut_exists(self, path: Path) -> bool:
        pass


class LinuxConfig(PlatformConfig):
    def shortcut_path(self, data_home: Path, name: str) -> Path:
        return data_home / ".local" / "share" / "applications" / f"{name}_{name}.desktop"

    def shortcut_exists(self, path: Path) -> bool:
        return path.is_file()


class MacOSConfig(PlatformConfig):
    def shortcut_path(self, data_home: Path, name: str) -> Path:
        return data_home / "Applications" / f"{name}.app"

    def shortcut_exists(self, path: Path) -> bool:
        return path.is_dir()


class WindowsConfig(PlatformConfig):
    def shortcut_path(self, data_home: Path, name: str) -> Path:
        return data_home / "Microsoft" / "Windows" / "Start Menu" / "Programs" / f"{name}.lnk"

    def shortcut_exists(self, path: Path) -> bool:
        return path.is_file()


def get_platform_config(platform: str) -> PlatformConfig:
    if platform == "linux-64":
        return LinuxConfig()
    elif platform in {"osx-arm64", "osx64"}:
        return MacOSConfig()
    elif platform == "win-64":
        return WindowsConfig()
    else:
        raise ValueError(f"Unsupported platform: {platform}")


def verify_shortcuts_exist(
    data_home: Path,
    shortcut_names: list[str],
    expected_exists: bool,
) -> None:
    """Verify if the specified shortcuts exist or not on the given system."""
    # Using the key to get the platform-specific configuration, to force a KeyError if the key is not found
    system = CURRENT_PLATFORM
    config = get_platform_config(system)
    for name in shortcut_names:
        shortcut_path = config.shortcut_path(data_home, name)
        exists = config.shortcut_exists(shortcut_path)
        assert exists == expected_exists, (
            f"Shortcut '{name}' {'should' if expected_exists else 'should not'} exist on {system}"
        )


def test_sync_creation_and_removal(
    pixi: Path,
    setup_data: SetupData,
    shortcuts_channel_1: str,
) -> None:
    """Test shortcut creation and removal with sync."""

    # Setup manifest with given shortcuts
    manifests = setup_data.pixi_home.joinpath("manifests")
    manifests.mkdir(parents=True)
    manifest = manifests.joinpath("pixi-global.toml")
    toml = f"""
    [envs.test]
    channels = ["{shortcuts_channel_1}"]
    dependencies = {{ pixi-editor = "*" }}
    """
    # Verify no shortcuts exist after sync
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=setup_data.env)
    verify_shortcuts_exist(setup_data.data_home, ["pixi-editor"], expected_exists=False)

    parsed_toml = tomllib.loads(toml)
    parsed_toml["envs"]["test"]["shortcuts"] = ["pixi-editor"]
    manifest.write_text(tomli_w.dumps(parsed_toml))

    # # Run sync and verify
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=setup_data.env)
    verify_shortcuts_exist(setup_data.data_home, ["pixi-editor"], expected_exists=True)

    # test removal of shortcuts
    del parsed_toml["envs"]["test"]["shortcuts"]
    manifest.write_text(tomli_w.dumps(parsed_toml))
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=setup_data.env)
    verify_shortcuts_exist(setup_data.data_home, ["pixi-editor"], expected_exists=False)


# TODO: test empty list of shortcuts
# TODO: test requesting shortcuts that are not available
# TODO: test that shortcuts are removed when environment is removed


def test_install_simple(
    pixi: Path,
    setup_data: SetupData,
    shortcuts_channel_1: str,
) -> None:
    """Test shortcut creation with install."""

    # Verify no shortcuts exist after sync
    verify_cli_command(
        [pixi, "global", "install", "--channel", shortcuts_channel_1, "pixi-editor"],
        ExitCode.SUCCESS,
        env=setup_data.env,
    )

    # Verify manifest
    manifest = setup_data.pixi_home.joinpath("manifests", "pixi-global.toml")
    parsed_toml = tomllib.loads(manifest.read_text())
    assert parsed_toml["envs"]["pixi-editor"]["shortcuts"] == ["pixi-editor"]

    # Verify shortcut exists
    verify_shortcuts_exist(setup_data.data_home, ["pixi-editor"], expected_exists=True)


def test_install_no_shortcut(
    pixi: Path,
    setup_data: SetupData,
    shortcuts_channel_1: str,
) -> None:
    """Test shortcut creation with install where `--no-shortcut` was passed."""

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
        env=setup_data.env,
    )

    # Verify manifest
    manifest = setup_data.pixi_home.joinpath("manifests", "pixi-global.toml")
    parsed_toml = tomllib.loads(manifest.read_text())
    assert "shortcuts" not in parsed_toml["envs"]["pixi-editor"]

    # Verify shortcut does not exist
    verify_shortcuts_exist(setup_data.data_home, ["pixi-editor"], expected_exists=False)
