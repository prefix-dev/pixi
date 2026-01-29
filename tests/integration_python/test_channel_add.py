from pathlib import Path

from .common import verify_cli_command


def test_channel_add_doesnt_update_packages(
    pixi: Path,
    tmp_pixi_workspace: Path,
    multiple_versions_channel_1: str,
    dummy_channel_1: str,
) -> None:
    """Test that adding channels does not cause the lockfile to be fully invalidated and packages to be updated.

    This test verifies the fix for issue #5077.

    The fix ensures that if a channel is appended the
    lockfile does not need to be completely regenerated.
    """
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = f"""
    [workspace]
    name = "test"
    channels = ["{multiple_versions_channel_1}"]
    platforms = ["linux-64", "osx-64", "osx-arm64", "win-64"]

    [dependencies]
    package = "==0.1.0"
    """
    manifest.write_text(toml)

    # Generate the lockfile
    verify_cli_command(
        [pixi, "list", "--manifest-path", manifest],
    )

    # Update the dependency to allow updates to be possible
    toml = toml.replace("==0.1.0", "*")
    manifest.write_text(toml)

    # Add another channel
    verify_cli_command(
        [pixi, "workspace", "channel", "add", dummy_channel_1, "--manifest-path", manifest],
    )

    # Verify that the package has not been updated
    verify_cli_command(
        [pixi, "list", "package", "--manifest-path", manifest],
        stdout_contains=["package", "0.1.0"],
    )
