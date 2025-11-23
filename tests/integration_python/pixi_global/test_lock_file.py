from pathlib import Path

import pytest
import yaml
from typing import Any, cast

from ..common import verify_cli_command

MANIFEST_VERSION = 1


def parse_lockfile(path: Path) -> dict[str, Any]:
    """Parse the global lockfile as YAML."""
    assert path.exists(), f"Lockfile {path} should exist"

    content = path.read_text()
    assert content.strip(), "Lockfile should not be empty"

    try:
        data = yaml.safe_load(content)
    except yaml.YAMLError as err:
        raise AssertionError(f"Lockfile is not valid YAML: {err}\nContent:\n{content}") from err

    assert isinstance(data, dict), "Lockfile YAML should decode to a dictionary"
    return cast(dict[str, Any], data)


@pytest.mark.slow
def test_global_lock_file_created(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    """Test that a lock file is created when installing a global package."""
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    lock_file = manifests.joinpath("pixi-global.lock")

    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_1, "dummy-a"],
        env=env,
    )

    # Must exist
    assert lock_file.exists(), "Lock file should be created after install"

    # Must parse successfully
    data = parse_lockfile(lock_file)
    assert isinstance(data, dict), "Lockfile root should be a dictionary"
    assert data, "Lockfile should contain structured data"


@pytest.mark.slow
def test_global_lock_file_reproducible(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    """Test that installations using lock file are reproducible."""
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    lock_file = manifests.joinpath("pixi-global.lock")

    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_1, "dummy-a"],
        env=env,
    )

    # Save lockfile contents
    original = lock_file.read_text()

    # Remove the environment directory to force re-creation
    env_dir = tmp_path.joinpath("envs", "dummy-a")
    if env_dir.exists():
        import shutil

        shutil.rmtree(env_dir)

    # Sync should use existing lockfile
    verify_cli_command([pixi, "global", "sync"], env=env)

    # Lockfile must not change after a reproducible sync
    assert lock_file.read_text() == original, "Lockfile should not change after sync"


@pytest.mark.slow
def test_global_lock_file_multiple_envs(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    """Ensure lockfile tracks multiple global environments."""
    env = {"PIXI_HOME": str(tmp_path)}
    lock_file = tmp_path.joinpath("manifests", "pixi-global.lock")

    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_1, "dummy-a"],
        env=env,
    )
    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_1, "dummy-b"],
        env=env,
    )

    data = parse_lockfile(lock_file)

    # Expect both environments represented in the lockfile
    environments = data.get("environments", {})
    assert "dummy-a" in environments
    assert "dummy-b" in environments


@pytest.mark.slow
def test_global_manifest_without_lock_file(
    pixi: Path, tmp_path: Path, dummy_channel_1: str
) -> None:
    """Pixi global should work if manifest exists but no lockfile is present."""
    env = {"PIXI_HOME": str(tmp_path)}

    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir(parents=True, exist_ok=True)

    lock_file = manifests.joinpath("pixi-global.lock")

    # Manually create manifest without lockfile
    manifest_content = f"""\
version = {MANIFEST_VERSION}

[envs.dummy-a]
channels = ["{dummy_channel_1}"]
dependencies = {{ dummy-a = "*" }}
exposed = {{ dummy-a = "dummy-a" }}
"""
    manifests.joinpath("pixi-global.toml").write_text(manifest_content)

    # This should create the lockfile
    verify_cli_command([pixi, "global", "sync"], env=env)

    # Now parse lockfile
    data = parse_lockfile(lock_file)
    assert "dummy-a" in data.get("environments", {}), "Lockfile must contain dummy-a"


@pytest.mark.slow
def test_global_lockfile_prevents_unexpected_version_changes(
    pixi: Path, tmp_path: Path, dummy_channel_1: str, dummy_channel_2: str
):
    """Lockfile should prevent version changes when newer packages are introduced."""

    env = {"PIXI_HOME": str(tmp_path)}
    lock_file = tmp_path.joinpath("manifests", "pixi-global.lock")

    # Install version from channel_1
    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_1, "dummy-a"],
        env=env,
    )
    first_lock = lock_file.read_text()

    # Replace environment directory to force reinstall
    env_dir = tmp_path.joinpath("envs", "dummy-a")
    import shutil

    shutil.rmtree(env_dir)

    # Now sync with channel_2 also available (which provides higher version)
    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_2, "dummy-a"],
        env=env,
    )

    # Must still use the locked version
    second_lock = lock_file.read_text()
    assert first_lock == second_lock, "Sync should not change locked package versions"


@pytest.mark.slow
def test_global_lockfile_contains_platform_entries(
    pixi: Path, tmp_path: Path, dummy_channel_1: str
):
    """Lockfile should record platform-specific metadata."""

    env = {"PIXI_HOME": str(tmp_path)}
    lock_file = tmp_path.joinpath("manifests", "pixi-global.lock")

    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_1, "dummy-a"],
        env=env,
    )

    data = parse_lockfile(lock_file)
    envs = data.get("environments", {})
    e = envs.get("dummy-a")

    assert e, "dummy-a environment should exist in lockfile"
    platforms = e.get("platforms", {}) or e.get("packages", {})

    # Depending on rattler_lock version, platform info may be nested differently
    assert platforms, "Platform records should exist in lockfile"


@pytest.mark.slow
def test_global_lockfile_respected_despite_channel_change(
    pixi: Path, tmp_path: Path, dummy_channel_1: str, dummy_channel_2: str
):
    """Lockfile resolution should not change even if new channels are added."""

    env = {"PIXI_HOME": str(tmp_path)}
    lock_file = tmp_path.joinpath("manifests", "pixi-global.lock")

    # Initial install from channel_1
    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_1, "dummy-a"],
        env=env,
    )
    original = lock_file.read_text()

    # Add another channel that may include different versions
    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_2, "dummy-a"],
        env=env,
    )

    # Lockfile should remain unchanged because lockfile pin is respected
    assert lock_file.read_text() == original, (
        "Adding new channels should not override lockfile resolution"
    )


@pytest.mark.slow
def test_global_lockfile_removes_dependency(pixi: Path, tmp_path: Path, dummy_channel_1: str):
    """Removing a dependency should remove it from the lockfile."""

    env = {"PIXI_HOME": str(tmp_path)}
    lock_file = tmp_path.joinpath("manifests", "pixi-global.lock")

    # Setup with two dependencies
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "dummy-a",
            "--with",
            "dummy-b",
        ],
        env=env,
    )
    verify_cli_command(
        [pixi, "global", "remove", "--environment", "dummy-a", "dummy-b"],
        env=env,
    )

    # Lockfile should have dummy-a but not dummy-b
    data = parse_lockfile(lock_file)
    env_data = str(data.get("environments", {}))

    assert "dummy-a" in env_data
    assert "dummy-b" not in env_data, "dummy-b should be removed from lockfile"


@pytest.mark.slow
def test_global_lockfile_updates_on_env_change(pixi: Path, tmp_path: Path, dummy_channel_1: str):
    """Changing dependencies should update the global lockfile."""

    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    lock_file = manifests.joinpath("pixi-global.lock")

    # Install initial package
    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_1, "dummy-a"],
        env=env,
    )
    original = lock_file.read_text()

    # Add a second package
    verify_cli_command(
        [pixi, "global", "add", "--environment", "dummy-a", "dummy-b"],
        env=env,
    )

    # Lockfile should have changed
    updated = lock_file.read_text()
    assert updated != original, "Lockfile should update when dependencies change"

    # Parsed lockfile should include both packages
    data = parse_lockfile(lock_file)
    pkg_list = str(data)
    assert "dummy-a" in pkg_list
    assert "dummy-b" in pkg_list
