from pathlib import Path
from typing import Any, cast
import shutil

import pytest
from rattler.lock import LockFile as _LockFile

from ..common import verify_cli_command

# Make pyright treat LockFile as Any so attribute access is allowed
LockFile = cast(Any, _LockFile)

MANIFEST_VERSION = 1


def parse_lockfile(path: Path) -> Any:
    """
    Load the global lockfile using py-rattler.

    Returns a LockFile instance (typed as Any for pyright's sake).
    """
    assert path.exists(), f"Lockfile {path} should exist"
    return LockFile.from_path(path)


def _package_names_for_env(lock_file: Any, env_name: str) -> set[str]:
    """Return the set of package names locked for a given environment."""
    env = lock_file.environment(env_name)
    if env is None:
        return set()

    names: set[str] = set()
    for platform in env.platforms():
        for pkg in env.packages(platform):
            name = getattr(pkg, "name", None)
            if isinstance(name, str):
                names.add(name)
    return names


def _locked_versions_for_package(lock_file: Any, env_name: str, package_name: str) -> set[str]:
    env = lock_file.environment(env_name)
    if env is None:
        return set()

    versions: set[str] = set()
    for platform in env.platforms():
        for pkg in env.packages(platform):
            if getattr(pkg, "name", None) != package_name:
                continue
            version = getattr(pkg, "version", None)
            if version is not None:
                versions.add(str(version))
    return versions


def _package_name_version_pairs(lock_file: Any, env_name: str) -> set[tuple[str, str]]:
    """Return {(name, version)} for all packages in a given environment."""
    env = lock_file.environment(env_name)
    if env is None:
        return set()

    pairs: set[tuple[str, str]] = set()
    for platform in env.platforms():
        for pkg in env.packages(platform):
            name = getattr(pkg, "name", None)
            version = getattr(pkg, "version", None)
            if isinstance(name, str) and version is not None:
                pairs.add((name, str(version)))
    return pairs


@pytest.mark.slow
def test_global_lock_file_created(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    """Test that a lock file is created when installing a global package."""
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    lock_file_path = manifests.joinpath("pixi-global.lock")

    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_1, "dummy-a"],
        env=env,
    )

    # Must exist
    assert lock_file_path.exists(), "Lock file should be created after install"

    # Must parse successfully and contain at least one environment
    lock_file = parse_lockfile(lock_file_path)
    env_names = [name for name, _ in lock_file.environments()]
    assert env_names, "Lockfile should contain at least one environment"


@pytest.mark.slow
def test_global_lock_file_reproducible(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    """Test that installations using lock file are reproducible."""
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    lock_file_path = manifests.joinpath("pixi-global.lock")

    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_1, "dummy-a"],
        env=env,
    )

    # Save lockfile contents
    original = lock_file_path.read_text()

    # Remove the environment directory to force re-creation
    env_dir = tmp_path.joinpath("envs", "dummy-a")
    if env_dir.exists():
        shutil.rmtree(env_dir)

    # Sync should use existing lockfile
    verify_cli_command([pixi, "global", "sync"], env=env)

    # Lockfile must not change after a reproducible sync
    assert lock_file_path.read_text() == original, "Lockfile should not change after sync"


@pytest.mark.slow
def test_global_lock_file_multiple_envs(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    """Ensure lockfile tracks multiple global environments."""
    env = {"PIXI_HOME": str(tmp_path)}
    lock_file_path = tmp_path.joinpath("manifests", "pixi-global.lock")

    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_1, "dummy-a"],
        env=env,
    )
    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_1, "dummy-b"],
        env=env,
    )

    lock_file = parse_lockfile(lock_file_path)
    env_names = {name for name, _ in lock_file.environments()}

    assert "dummy-a" in env_names
    assert "dummy-b" in env_names


@pytest.mark.slow
def test_global_manifest_without_lock_file(
    pixi: Path, tmp_path: Path, dummy_channel_1: str
) -> None:
    """Pixi global should work if manifest exists but no lockfile is present."""
    env = {"PIXI_HOME": str(tmp_path)}

    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir(parents=True, exist_ok=True)

    lock_file_path = manifests.joinpath("pixi-global.lock")

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
    lock_file = parse_lockfile(lock_file_path)
    env_names = {name for name, _ in lock_file.environments()}
    assert "dummy-a" in env_names, "Lockfile must contain dummy-a"


@pytest.mark.slow
def test_global_lockfile_prevents_unexpected_version_changes(
    pixi: Path, tmp_path: Path, dummy_channel_1: str, dummy_channel_2: str
) -> None:
    """Lockfile should prevent version changes when newer packages are introduced."""
    env = {"PIXI_HOME": str(tmp_path)}
    lock_file_path = tmp_path.joinpath("manifests", "pixi-global.lock")

    # Install version from channel_1
    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_1, "dummy-a"],
        env=env,
    )
    first_lock = lock_file_path.read_text()

    # Replace environment directory to force reinstall
    env_dir = tmp_path.joinpath("envs", "dummy-a")
    shutil.rmtree(env_dir)

    # Now sync with channel_2 also available (which provides higher version)
    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_2, "dummy-a"],
        env=env,
    )

    # Must still use the locked version
    second_lock = lock_file_path.read_text()
    assert first_lock == second_lock, "Sync should not change locked package versions"


@pytest.mark.slow
def test_global_lockfile_contains_platform_entries(
    pixi: Path, tmp_path: Path, dummy_channel_1: str
) -> None:
    """Lockfile should record platform-specific metadata."""
    env = {"PIXI_HOME": str(tmp_path)}
    lock_file_path = tmp_path.joinpath("manifests", "pixi-global.lock")

    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_1, "dummy-a"],
        env=env,
    )

    lock_file = parse_lockfile(lock_file_path)
    env_obj = lock_file.environment("dummy-a")
    assert env_obj is not None, "dummy-a environment should exist in lockfile"

    # There must be at least one platform registered.
    assert env_obj.platforms(), "Platform records should exist in lockfile"


@pytest.mark.slow
def test_global_lockfile_respected_despite_channel_change(
    pixi: Path, tmp_path: Path, dummy_channel_1: str, dummy_channel_2: str
) -> None:
    """Lockfile resolution should not change even if new channels are added."""
    env = {"PIXI_HOME": str(tmp_path)}
    lock_file_path = tmp_path.joinpath("manifests", "pixi-global.lock")

    # Initial install from channel_1
    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_1, "dummy-a"],
        env=env,
    )
    original = lock_file_path.read_text()

    # Add another channel that may include different versions
    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_2, "dummy-a"],
        env=env,
    )

    # Lockfile should remain unchanged because lockfile pin is respected
    assert lock_file_path.read_text() == original, (
        "Adding new channels should not override lockfile resolution"
    )


@pytest.mark.slow
def test_global_lockfile_removes_dependency(pixi: Path, tmp_path: Path, dummy_channel_1: str):
    """Removing a dependency should remove it from the lockfile."""
    env = {"PIXI_HOME": str(tmp_path)}
    lock_file_path = tmp_path.joinpath("manifests", "pixi-global.lock")

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

    lock_file = parse_lockfile(lock_file_path)
    env_names = {name for name, _ in lock_file.environments()}
    assert "dummy-a" in env_names

    names = _package_names_for_env(lock_file, "dummy-a")
    assert "dummy-b" not in names, "dummy-b should be removed from lockfile"


@pytest.mark.slow
def test_global_lockfile_updates_on_env_change(pixi: Path, tmp_path: Path, dummy_channel_1: str):
    """Changing dependencies should update the global lockfile."""
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    lock_file_path = manifests.joinpath("pixi-global.lock")

    # Install initial package
    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_1, "dummy-a"],
        env=env,
    )
    original_text = lock_file_path.read_text()

    # Add a second package
    verify_cli_command(
        [pixi, "global", "add", "--environment", "dummy-a", "dummy-b"],
        env=env,
    )

    # Lockfile should have changed
    updated_text = lock_file_path.read_text()
    assert updated_text != original_text, "Lockfile should update when dependencies change"

    # Parsed lockfile should include both packages for env dummy-a
    lock_file = parse_lockfile(lock_file_path)
    names = _package_names_for_env(lock_file, "dummy-a")
    assert "dummy-a" in names
    assert "dummy-b" in names


@pytest.mark.slow
def test_global_lockfile_updates_package_version_when_relocked(
    pixi: Path, tmp_path: Path, dummy_channel_1: str, dummy_channel_2: str
) -> None:
    """
    If we drop the global lockfile and reinstall, pixi should re-solve and
    produce a lockfile that is semantically consistent with the original one:
    the set of locked (name, version) pairs for dummy-a should remain stable.

    This guards against accidental changes in the re-locking path, even if
    additional channels are provided.
    """
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path / "manifests"
    lock_file_path = manifests / "pixi-global.lock"

    # Initial install from channel_1
    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_1, "dummy-a"],
        env=env,
    )

    lock_initial = parse_lockfile(lock_file_path)
    initial_pairs = _package_name_version_pairs(lock_initial, "dummy-a")
    assert initial_pairs, "dummy-a should have at least one locked package initially"

    # Remove lockfile and env prefix so that a fresh solve happens
    if lock_file_path.exists():
        lock_file_path.unlink()

    env_dir = tmp_path.joinpath("envs", "dummy-a")
    if env_dir.exists():
        shutil.rmtree(env_dir)

    # Reinstall, this time also passing dummy_channel_2. Current pixi behavior
    # keeps using the manifest's channels, so we don't assert that the channel
    # set changes. Only that the resolved package set stays consistent.
    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_2, "dummy-a"],
        env=env,
    )

    lock_updated = parse_lockfile(lock_file_path)
    updated_pairs = _package_name_version_pairs(lock_updated, "dummy-a")
    assert updated_pairs, "dummy-a should still have locked packages after re-locking"

    # The important invariant: re-locking from scratch keeps the same package
    # versions for the environment.
    assert updated_pairs == initial_pairs, (
        "Re-solving after removing the lockfile should preserve the locked "
        "package versions for dummy-a"
    )


@pytest.mark.slow
def test_global_lockfile_removes_dependency_structurally(
    pixi: Path, tmp_path: Path, dummy_channel_1: str
) -> None:
    """Removing a dependency should remove it from the lockfile's package list."""
    env = {"PIXI_HOME": str(tmp_path)}
    lock_file_path = tmp_path.joinpath("manifests", "pixi-global.lock")

    # Setup with dummy-a plus dummy-b
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

    lock_before = parse_lockfile(lock_file_path)
    versions_before = _locked_versions_for_package(lock_before, "dummy-a", "dummy-b")
    assert versions_before, "dummy-b should be present before removal"

    # Remove dummy-b from environment dummy-a
    verify_cli_command(
        [pixi, "global", "remove", "--environment", "dummy-a", "dummy-b"],
        env=env,
    )

    lock_after = parse_lockfile(lock_file_path)
    versions_after = _locked_versions_for_package(lock_after, "dummy-a", "dummy-b")

    assert not versions_after, "dummy-b should be removed from the lockfile packages"


@pytest.mark.slow
def test_global_lockfile_populates_missing_env_from_existing_prefix(
    pixi: Path, tmp_path: Path, dummy_channel_1: str
) -> None:
    """
    If the manifest defines multiple envs and their prefixes exist on disk,
    but the global lockfile is missing one of them (e.g. old Pixi version),
    `pixi global sync` should add the missing env to the lockfile and leave
    existing env entries untouched.
    """
    # Primary PIXI_HOME (the one whose lockfile we care about)
    home = tmp_path
    manifests = home / "manifests"
    lock_file_path = manifests / "pixi-global.lock"
    env_primary = {"PIXI_HOME": str(home)}

    # Create a single global environment "dummy-a" in the primary home.
    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_1, "dummy-a"],
        env=env_primary,
    )

    # Snapshot lockfile state for dummy-a so we can assert it stays unchanged.
    full_lock = parse_lockfile(lock_file_path)
    env_names_full = {name for name, _ in full_lock.environments()}
    assert "dummy-a" in env_names_full

    dummy_a_env = full_lock.environment("dummy-a")
    assert dummy_a_env is not None

    dummy_a_channels_before = [str(ch) for ch in dummy_a_env.channels()]
    dummy_a_pkgs_before = _package_name_version_pairs(full_lock, "dummy-a")

    # Create a *separate* PIXI_HOME where we install dummy-b.
    # We'll steal its prefix to simulate an existing prefix for an env that
    # is NOT yet in the primary lockfile.
    donor_home = tmp_path / "donor_home"
    env_donor = {"PIXI_HOME": str(donor_home)}

    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_1, "dummy-b"],
        env=env_donor,
    )

    donor_env_dir = donor_home / "envs" / "dummy-b"
    assert donor_env_dir.exists(), "Donor prefix for dummy-b should exist"

    # Copy donor prefix into the primary PIXI_HOME as env "dummy-b".
    primary_envs_root = home / "envs"
    target_env_dir = primary_envs_root / "dummy-b"
    if target_env_dir.exists():
        shutil.rmtree(target_env_dir)
    shutil.copytree(donor_env_dir, target_env_dir)

    # Extend the *primary* global manifest to define envs.dummy-b.
    manifest_path = manifests / "pixi-global.toml"
    manifest_text = manifest_path.read_text()
    manifest_text += (
        f"\n[envs.dummy-b]\n"
        f'channels = ["{dummy_channel_1}"]\n'
        f'dependencies = {{ dummy-b = "*" }}\n'
        f'exposed = {{ dummy-b = "dummy-b" }}\n'
    )
    manifest_path.write_text(manifest_text)

    # At this point:
    # - manifest has dummy-a and dummy-b
    # - prefixes on disk: dummy-a and dummy-b
    # - lockfile only has dummy-a (no entry for dummy-b)
    before_sync_lock = parse_lockfile(lock_file_path)
    env_names_before = {name for name, _ in before_sync_lock.environments()}
    assert env_names_before == {"dummy-a"}, "Lockfile should only know about dummy-a before sync"

    # Run `pixi global sync` in the primary home. This should call
    # populate_missing_lock_environments_from_existing_prefix and synthesize
    # lock entries for dummy-b based on its existing prefix.
    verify_cli_command([pixi, "global", "sync"], env=env_primary)

    final_lock = parse_lockfile(lock_file_path)
    env_names_after = {name for name, _ in final_lock.environments()}

    # Existing env must still be there, and missing env must be added.
    assert "dummy-a" in env_names_after, "Existing env entry must be preserved"
    assert "dummy-b" in env_names_after, "Missing env should be synthesized from prefix"

    # 5. Verify that dummy-a's lock data is unchanged (channels + {name, version} set).
    dummy_a_env_after = final_lock.environment("dummy-a")
    assert dummy_a_env_after is not None

    dummy_a_channels_after = [str(ch) for ch in dummy_a_env_after.channels()]
    dummy_a_pkgs_after = _package_name_version_pairs(final_lock, "dummy-a")

    assert dummy_a_channels_after == dummy_a_channels_before, (
        "Channels for existing env should not change when populating missing envs"
    )
    assert dummy_a_pkgs_after == dummy_a_pkgs_before, (
        "Locked packages for existing env should remain unchanged"
    )


@pytest.mark.slow
def test_global_lockfile_ignores_prefix_without_manifest_env(
    pixi: Path, tmp_path: Path, dummy_channel_1: str
) -> None:
    """
    `pixi global sync` should not create lockfile entries for prefixes that
    exist on disk but are not referenced by the global manifest.
    """
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path / "manifests"
    lock_file_path = manifests / "pixi-global.lock"

    # Create a single global env dummy-a
    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_1, "dummy-a"],
        env=env,
    )

    lock_before = parse_lockfile(lock_file_path)
    env_names_before = {name for name, _ in lock_before.environments()}
    assert env_names_before == {"dummy-a"}

    # Clone dummy-a's prefix to create an "orphan" prefix not in the manifest
    envs_root = tmp_path / "envs"
    dummy_a_dir = envs_root / "dummy-a"
    orphan_dir = envs_root / "orphan-env"
    shutil.copytree(dummy_a_dir, orphan_dir)

    # Sanity: manifest should still only describe dummy-a, not orphan-env
    manifest_path = manifests / "pixi-global.toml"
    manifest_text = manifest_path.read_text()
    assert "dummy-a" in manifest_text
    assert "orphan-env" not in manifest_text

    # Sync should NOT add orphan-env to the lockfile
    verify_cli_command([pixi, "global", "sync"], env=env)

    lock_after = parse_lockfile(lock_file_path)
    env_names_after = {name for name, _ in lock_after.environments()}

    assert "dummy-a" in env_names_after
    assert "orphan-env" not in env_names_after, (
        "Prefixes without a corresponding manifest env must not appear in the lockfile"
    )
