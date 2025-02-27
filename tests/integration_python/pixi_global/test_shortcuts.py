from pathlib import Path
import tomllib

import pytest
import tomli_w
from ..common import verify_cli_command, ExitCode
import platform


@pytest.mark.skipif(platform.system() != "Linux", reason="Only runs on Linux")
def test_sync_shortcuts_linux(pixi: Path, tmp_path: Path, shortcuts_channel_1: str) -> None:
    pixi_home = tmp_path / "pixi_home"
    data_home = tmp_path / "data_home"
    env = {"PIXI_HOME": str(pixi_home), "XDG_DATA_HOME": str(data_home)}
    manifests = pixi_home.joinpath("manifests")
    manifests.mkdir(parents=True)
    manifest = manifests.joinpath("pixi-global.toml")
    toml = f"""
    [envs.test]
    channels = ["{shortcuts_channel_1}"]
    dependencies = {{ pixi-editor = "*" }}
    """
    parsed_toml = tomllib.loads(toml)
    manifest.write_text(toml)

    desktop_file = data_home.joinpath("applications", "pixi-editor_pixi-editor.desktop")

    # Run sync and assert that no shortcuts are created
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=env)
    assert not desktop_file.is_file()

    # Enable shortcuts for pixi-editor
    parsed_toml["envs"]["test"]["shortcuts"] = ["pixi-editor"]
    manifest.write_text(tomli_w.dumps(parsed_toml))

    # Now shortcuts should be created
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=env)
    assert desktop_file.is_file()

    # Remove shortcuts again
    del parsed_toml["envs"]["test"]["shortcuts"]
    manifest.write_text(tomli_w.dumps(parsed_toml))

    # Shortcuts should be removed again
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=env)
    assert not desktop_file.is_file()
