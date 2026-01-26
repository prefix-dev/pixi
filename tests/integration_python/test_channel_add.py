from pathlib import Path

from .common import verify_cli_command


def test_channel_add_doesnt_update_packages(
    pixi: Path,
    tmp_pixi_workspace: Path,
) -> None:
    """Test that adding channels does not cause the lockfile to be fully invalidated and packages to be updated.

    This test verifies the fix for issue #5077.

    The fix ensures that if a channel is appended the
    lockfile does not need to be completely regenerated.
    """
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = """
    [workspace]
    name = "test"
    channels = ["conda-forge"]
    platforms = ["linux-64", "osx-64", "osx-arm64", "win-64"]

    [dependencies]
    crane = "==0.20.0"
    """
    manifest.write_text(toml)

    # Generate the lockfile
    verify_cli_command(
        [pixi, "list", "--manifest-path", manifest],
    )

    # Update the dependency to allow updates to be possible
    toml = toml.replace("==0.20.0", "*")
    manifest.write_text(toml)

    # Add another channel
    verify_cli_command(
        [pixi, "workspace", "channel", "add", "bioconda", "--manifest-path", manifest],
    )

    # Verify that crane has not been updated
    verify_cli_command(
        [pixi, "list", "crane", "--manifest-path", manifest],
        stdout_contains=["crane", "0.20.0"],
    )
