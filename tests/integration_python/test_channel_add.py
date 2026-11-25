from pathlib import Path

import tomli

from .common import CURRENT_PLATFORM, verify_cli_command


def test_channel_add_doesnt_update_packages(
    pixi: Path,
    tmp_pixi_workspace: Path,
    multiple_versions_channel_1: str,
    dummy_channel_1: str,
) -> None:
    """Test that adding channels does not cause the lock file to be fully invalidated and packages to be updated.

    This test verifies the fix for issue #5077.

    The fix ensures that if a channel is appended the
    lock file does not need to be completely regenerated.
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

    # Generate the lock file
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


def test_channel_prepend(
    pixi: Path,
    tmp_pixi_workspace: Path,
    dummy_channel_1: str,
    dummy_channel_2: str,
    multiple_versions_channel_1: str,
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    manifest.write_text(f"""
    [workspace]
    name = "test"
    channels = ["{dummy_channel_1}", "{dummy_channel_2}"]
    platforms = ["{CURRENT_PLATFORM}"]

    [feature.extra]
    channels = ["{dummy_channel_1}"]
    """)

    def channels() -> tuple[list[str], list[str]]:
        parsed = tomli.loads(manifest.read_text())
        return parsed["workspace"]["channels"], parsed["feature"]["extra"]["channels"]

    def run(*args: str) -> str:
        return verify_cli_command(
            [pixi, "workspace", "channel", *args, "--manifest-path", manifest, "--no-install"],
        ).stderr

    # Existing channels move instead of being duplicated, new ones are inserted.
    stderr = run("prepend", multiple_versions_channel_1, dummy_channel_2)
    assert "deprecated" not in stderr
    assert f"Added {multiple_versions_channel_1}" in stderr
    assert f"{dummy_channel_2} is already at the front" in stderr
    assert channels() == (
        [multiple_versions_channel_1, dummy_channel_2, dummy_channel_1],
        [dummy_channel_1],
    )

    # The deprecated spelling behaves like `prepend` and warns once.
    stderr = run("add", "--prepend", dummy_channel_1)
    warnings = [line for line in stderr.splitlines() if "deprecated" in line]
    assert len(warnings) == 1
    assert f"Moved {dummy_channel_1} to the front" in stderr
    assert warnings[0].endswith(
        "the '--prepend' option is deprecated; use 'pixi workspace channel prepend' instead"
    )
    assert channels() == (
        [dummy_channel_1, multiple_versions_channel_1, dummy_channel_2],
        [dummy_channel_1],
    )

    # Adding an existing channel leaves it in place.
    stderr = run("add", dummy_channel_2)
    assert f"{dummy_channel_2} is already listed" in stderr
    assert "Added" not in stderr
    assert channels() == (
        [dummy_channel_1, multiple_versions_channel_1, dummy_channel_2],
        [dummy_channel_1],
    )

    # Prepending to a feature leaves the workspace channels alone.
    run("prepend", "--feature", "extra", dummy_channel_2)
    assert channels() == (
        [dummy_channel_1, multiple_versions_channel_1, dummy_channel_2],
        [dummy_channel_2, dummy_channel_1],
    )
