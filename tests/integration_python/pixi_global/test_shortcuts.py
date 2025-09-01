import hashlib
import tomllib
from abc import ABC, abstractmethod
from dataclasses import dataclass
from pathlib import Path

import pytest
import tomli_w

from ..common import CURRENT_PLATFORM, ExitCode, verify_cli_command


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
    def shortcut_exists(self, data_home: Path, name: str) -> bool:
        """Given the name of a shortcut, return whether it exists or not."""
        pass

    @abstractmethod
    def get_shortcut_content_hash(self, data_home: Path, name: str) -> str:
        """Get a hash of the shortcut content for comparison. Raises FileNotFoundError if shortcut doesn't exist."""
        pass


class LinuxConfig(PlatformConfig):
    def shortcut_path(self, data_home: Path, name: str) -> Path:
        return data_home / ".local" / "share" / "applications" / f"{name}_{name}.desktop"

    def shortcut_exists(self, data_home: Path, name: str) -> bool:
        return self.shortcut_path(data_home, name).is_file()

    def get_shortcut_content_hash(self, data_home: Path, name: str) -> str:
        shortcut_file = self.shortcut_path(data_home, name)
        if not shortcut_file.is_file():
            raise FileNotFoundError(
                f"Shortcut file {shortcut_file} does not exist or is not a file"
            )
        return hashlib.sha256(shortcut_file.read_bytes()).hexdigest()


class MacOSConfig(PlatformConfig):
    def shortcut_path(self, data_home: Path, name: str) -> Path:
        return data_home / "Applications" / f"{name}.app"

    def shortcut_exists(self, data_home: Path, name: str) -> bool:
        return self.shortcut_path(data_home, name).is_dir()

    def get_shortcut_content_hash(self, data_home: Path, name: str) -> str:
        shortcut_dir = self.shortcut_path(data_home, name)
        if not shortcut_dir.is_dir():
            raise FileNotFoundError(
                f"Shortcut directory {shortcut_dir} does not exist or is not a directory"
            )

        # Hash all files in the .app directory recursively
        hash_md5 = hashlib.sha256()
        for file_path in sorted(shortcut_dir.rglob("*")):
            if file_path.is_file():
                hash_md5.update(file_path.read_bytes())
        return hash_md5.hexdigest()


class WindowsConfig(PlatformConfig):
    def shortcut_path(self, data_home: Path, name: str) -> Path:
        return data_home / "Desktop" / f"{name}.lnk"

    def shortcut_exists(self, data_home: Path, name: str) -> bool:
        return self.shortcut_path(data_home, name).is_file()

    def get_shortcut_content_hash(self, data_home: Path, name: str) -> str:
        shortcut_file = self.shortcut_path(data_home, name)
        if not shortcut_file.is_file():
            raise FileNotFoundError(
                f"Shortcut file {shortcut_file} does not exist or is not a file"
            )
        return hashlib.sha256(shortcut_file.read_bytes()).hexdigest()


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
        assert config.shortcut_exists(data_home, name) == expected_exists, (
            f"Shortcut '{name}' {'should' if expected_exists else 'should not'} exist on {system}"
        )


def get_shortcut_content_hash(data_home: Path, name: str) -> str:
    """Get the hash of a shortcut's content for the current platform."""
    system = CURRENT_PLATFORM
    config = get_platform_config(system)
    return config.get_shortcut_content_hash(data_home, name)


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
    manifest.write_text(toml)

    # Verify no shortcuts exist after sync
    verify_cli_command([pixi, "global", "sync"], env=setup_data.env)
    verify_shortcuts_exist(setup_data.data_home, ["pixi-editor"], expected_exists=False)

    parsed_toml = tomllib.loads(toml)
    parsed_toml["envs"]["test"]["shortcuts"] = ["pixi-editor"]
    manifest.write_text(tomli_w.dumps(parsed_toml))

    # # Run sync and verify
    verify_cli_command([pixi, "global", "sync"], env=setup_data.env)
    verify_shortcuts_exist(setup_data.data_home, ["pixi-editor"], expected_exists=True)

    # test removal of shortcuts
    del parsed_toml["envs"]["test"]["shortcuts"]
    manifest.write_text(tomli_w.dumps(parsed_toml))
    verify_cli_command([pixi, "global", "sync"], env=setup_data.env)
    verify_shortcuts_exist(setup_data.data_home, ["pixi-editor"], expected_exists=False)


def test_sync_empty_shortcut_list(
    pixi: Path,
    setup_data: SetupData,
    shortcuts_channel_1: str,
) -> None:
    # Setup manifest with given shortcuts
    manifests = setup_data.pixi_home.joinpath("manifests")
    manifests.mkdir(parents=True)
    manifest = manifests.joinpath("pixi-global.toml")
    toml = f"""
    [envs.test]
    channels = ["{shortcuts_channel_1}"]
    dependencies = {{ pixi-editor = "*" }}
    shortcuts = ["pixi-editor"]
    """
    manifest.write_text(toml)

    # Run sync and verify
    verify_cli_command([pixi, "global", "sync"], env=setup_data.env)
    verify_shortcuts_exist(setup_data.data_home, ["pixi-editor"], expected_exists=True)

    # Set shortcuts to empty list
    parsed_toml = tomllib.loads(toml)
    parsed_toml["envs"]["test"]["shortcuts"] = []
    manifest.write_text(tomli_w.dumps(parsed_toml))
    verify_cli_command([pixi, "global", "sync"], env=setup_data.env)
    verify_shortcuts_exist(setup_data.data_home, ["pixi-editor"], expected_exists=False)


def test_sync_removing_environment(
    pixi: Path,
    setup_data: SetupData,
    shortcuts_channel_1: str,
) -> None:
    # Setup manifest with given shortcuts
    manifests = setup_data.pixi_home.joinpath("manifests")
    manifests.mkdir(parents=True)
    manifest = manifests.joinpath("pixi-global.toml")
    toml = f"""
    [envs.test]
    channels = ["{shortcuts_channel_1}"]
    dependencies = {{ pixi-editor = "*" }}
    shortcuts = ["pixi-editor"]
    """
    manifest.write_text(toml)

    # Run sync and verify
    verify_cli_command([pixi, "global", "sync"], env=setup_data.env)
    verify_shortcuts_exist(setup_data.data_home, ["pixi-editor"], expected_exists=True)

    # Remove environment
    parsed_toml = tomllib.loads(toml)
    del parsed_toml["envs"]["test"]
    manifest.write_text(tomli_w.dumps(parsed_toml))
    verify_cli_command([pixi, "global", "sync"], env=setup_data.env)
    verify_shortcuts_exist(setup_data.data_home, ["pixi-editor"], expected_exists=False)


def test_sync_duplicate_shortcuts(
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
    [envs.test1]
    channels = ["{shortcuts_channel_1}"]
    dependencies = {{ pixi-editor = "*" }}
    shortcuts = ["pixi-editor"]

    [envs.test2]
    channels = ["{shortcuts_channel_1}"]
    dependencies = {{ pixi-editor = "*" }}
    shortcuts = ["pixi-editor"]
    """
    manifest.write_text(toml)

    # Verify no shortcuts exist after sync
    verify_cli_command(
        [pixi, "global", "sync"],
        ExitCode.FAILURE,
        env=setup_data.env,
        stderr_contains="Duplicated shortcut names found: pixi-editor",
    )


def test_sync_unavailable_shortcuts(
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
    [envs.test1]
    channels = ["{shortcuts_channel_1}"]
    dependencies = {{ pixi-editor = "*" }}
    shortcuts = ["unavailable-shortcut"]
    """
    manifest.write_text(toml)

    # Verify no shortcuts exist after sync
    verify_cli_command(
        [pixi, "global", "sync"],
        ExitCode.FAILURE,
        env=setup_data.env,
        stderr_contains="the following shortcuts are requested but not available: unavailable-shortcut",
    )


def test_install_simple(
    pixi: Path,
    setup_data: SetupData,
    shortcuts_channel_1: str,
) -> None:
    """Test shortcut creation with install."""

    # Verify no shortcuts exist after sync
    verify_cli_command(
        [pixi, "global", "install", "--channel", shortcuts_channel_1, "pixi-editor"],
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
        env=setup_data.env,
    )

    # Verify manifest
    manifest = setup_data.pixi_home.joinpath("manifests", "pixi-global.toml")
    parsed_toml = tomllib.loads(manifest.read_text())
    assert "shortcuts" not in parsed_toml["envs"]["pixi-editor"]

    # Verify shortcut does not exist
    verify_shortcuts_exist(setup_data.data_home, ["pixi-editor"], expected_exists=False)


def test_remove_shortcut(
    pixi: Path,
    setup_data: SetupData,
    shortcuts_channel_1: str,
) -> None:
    # Verify no shortcuts exist after sync
    verify_cli_command(
        [pixi, "global", "install", "--channel", shortcuts_channel_1, "pixi-editor"],
        env=setup_data.env,
    )

    # Verify manifest
    manifest = setup_data.pixi_home.joinpath("manifests", "pixi-global.toml")
    parsed_toml = tomllib.loads(manifest.read_text())
    assert parsed_toml["envs"]["pixi-editor"]["shortcuts"] == ["pixi-editor"]

    # Verify shortcut exists
    verify_shortcuts_exist(setup_data.data_home, ["pixi-editor"], expected_exists=True)

    # Remove shortcut
    verify_cli_command(
        [pixi, "global", "shortcut", "remove", "pixi-editor"],
        env=setup_data.env,
    )

    # Verify removal from manifest
    parsed_toml = tomllib.loads(manifest.read_text())
    assert parsed_toml["envs"]["pixi-editor"]["shortcuts"] != ["pixi-editor"]

    # Verify shortcut does not exist
    verify_shortcuts_exist(setup_data.data_home, ["pixi-editor"], expected_exists=False)


def test_add_shortcut(
    pixi: Path,
    setup_data: SetupData,
    shortcuts_channel_1: str,
) -> None:
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--no-shortcuts",
            "--channel",
            shortcuts_channel_1,
            "pixi-editor",
        ],
        env=setup_data.env,
    )

    # Verify manifest
    manifest = setup_data.pixi_home.joinpath("manifests", "pixi-global.toml")
    parsed_toml = tomllib.loads(manifest.read_text())
    assert parsed_toml["envs"]["pixi-editor"].get("shortcuts") is None

    # Verify shortcut exists
    verify_shortcuts_exist(setup_data.data_home, ["pixi-editor"], expected_exists=False)

    # Add shortcut
    verify_cli_command(
        [pixi, "global", "shortcut", "add", "pixi-editor", "--environment", "pixi-editor"],
        env=setup_data.env,
    )

    # Verify addition to manifest
    parsed_toml = tomllib.loads(manifest.read_text())
    assert parsed_toml["envs"]["pixi-editor"]["shortcuts"] == ["pixi-editor"]

    # Verify shortcut exists
    verify_shortcuts_exist(setup_data.data_home, ["pixi-editor"], expected_exists=True)


def test_update_installs_new_shortcuts(
    pixi: Path,
    setup_data: SetupData,
    shortcuts_channel_1: str,
) -> None:
    # Verify shortcuts exist after sync
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            shortcuts_channel_1,
            "pixi-editor=1.0.0",
        ],
        env=setup_data.env,
    )
    verify_shortcuts_exist(setup_data.data_home, ["pixi-editor"], expected_exists=True)

    # Change version requirement to '*'
    manifests = setup_data.pixi_home.joinpath("manifests")
    manifest = manifests.joinpath("pixi-global.toml")
    manifest_dict = tomllib.loads(manifest.read_text())
    manifest_dict["envs"]["pixi-editor"]["dependencies"]["pixi-editor"] = "*"
    manifest.write_text(tomli_w.dumps(manifest_dict))

    # Get initial hash before sync
    initial_hash = get_shortcut_content_hash(setup_data.data_home, "pixi-editor")

    # Run pixi sync (nothing should be updated here)
    verify_cli_command([pixi, "global", "sync"], env=setup_data.env)
    verify_shortcuts_exist(setup_data.data_home, ["pixi-editor"], expected_exists=True)

    # Verify shortcut content is unchanged
    current_hash = get_shortcut_content_hash(setup_data.data_home, "pixi-editor")
    assert current_hash == initial_hash, "Shortcut content should remain unchanged after sync"

    # Run pixi update
    verify_cli_command([pixi, "global", "update"], env=setup_data.env)
    verify_shortcuts_exist(setup_data.data_home, ["pixi-editor"], expected_exists=True)

    # Verify shortcut content has changed after update
    updated_hash = get_shortcut_content_hash(setup_data.data_home, "pixi-editor")
    assert updated_hash != initial_hash, "Shortcut content should be updated after update command"
