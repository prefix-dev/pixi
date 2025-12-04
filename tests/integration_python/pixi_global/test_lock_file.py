import json
import shutil
import tomllib
from pathlib import Path
from typing import Any, cast

import pytest
import tomli_w
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


def _get_conda_meta_version(env_dir: Path, package_name: str) -> str | None:
    """Read the installed version of a package from conda-meta."""
    conda_meta = env_dir / "conda-meta"
    if not conda_meta.exists():
        return None
    for json_file in conda_meta.glob(f"{package_name}-*.json"):
        data = json.loads(json_file.read_text())
        return data.get("version")
    return None


@pytest.mark.slow
def test_global_lock_file_reproducible(
    pixi: Path, tmp_path: Path, multiple_versions_channel_1: str
) -> None:
    """Test that installations using lock file are reproducible.

    This test verifies that:
    1. When a lockfile exists, sync respects the locked version
    2. When the lockfile is removed, a fresh solve gets the latest version
    """
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifest_path = manifests.joinpath("pixi-global.toml")
    lock_file_path = manifests.joinpath("pixi-global.lock")
    env_dir = tmp_path.joinpath("envs", "package2")

    # Step 1: Install a specific lower version (0.1.0)
    verify_cli_command(
        [pixi, "global", "install", "--channel", multiple_versions_channel_1, "package2==0.1.0"],
        env=env,
    )

    # Verify we got the expected version
    lock_file = parse_lockfile(lock_file_path)
    versions = _locked_versions_for_package(lock_file, "package2", "package2")
    assert "0.1.0" in versions, "Should have locked version 0.1.0"

    # Modify the manifest to use "*" as version spec
    manifest_data = tomllib.loads(manifest_path.read_text())
    manifest_data["envs"]["package2"]["dependencies"]["package2"] = "*"
    manifest_path.write_bytes(tomli_w.dumps(manifest_data).encode())

    # Remove the environment directory to force re-creation
    if env_dir.exists():
        shutil.rmtree(env_dir)

    # Sync should use existing lockfile and install 0.1.0
    verify_cli_command([pixi, "global", "sync"], env=env)

    # Verify the installed version from conda-meta is still 0.1.0
    installed_version = _get_conda_meta_version(env_dir, "package2")
    assert installed_version == "0.1.0", (
        f"Sync with lockfile should install locked version 0.1.0, got {installed_version}"
    )

    # Remove the lockfile and env dir
    lock_file_path.unlink()
    shutil.rmtree(env_dir)

    # Sync without lockfile should do a fresh solve and get 0.2.0
    verify_cli_command([pixi, "global", "sync"], env=env)

    # Verify we now have the newer version
    new_installed_version = _get_conda_meta_version(env_dir, "package2")
    assert new_installed_version == "0.2.0", (
        f"Sync without lockfile should install latest version 0.2.0, got {new_installed_version}"
    )


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
