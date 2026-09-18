from pathlib import Path

import yaml

from .common import CURRENT_PLATFORM, ExitCode, verify_cli_command


def test_local_resolution_tracks_manifest(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace / "pixi.toml"
    manifest.write_text(f'''[workspace]
name = "local"
channels = ["{dummy_channel_1}"]
platforms = []
[dependencies]
dummy-a = "*"
''')
    args = ["--manifest-path", manifest]
    lock = tmp_pixi_workspace / ".pixi" / "locks" / "default" / f"{CURRENT_PLATFORM}.lock"
    verify_cli_command([pixi, "install", *args])
    assert not (tmp_pixi_workspace / "pixi.lock").exists()
    data = yaml.safe_load(lock.read_text())
    assert set(data["environments"]) == {"default"}
    assert len(data["platforms"]) == 1
    before = lock.read_bytes()
    verify_cli_command([pixi, "install", "--locked", *args])
    assert lock.read_bytes() == before
    verify_cli_command(
        [pixi, "workspace", "export", "conda-environment", "--from-lock-file", *args],
        stdout_contains="dummy-a",
    )

    # Hand edits must invalidate both the local resolution and installed prefix.
    manifest.write_text(manifest.read_text() + 'dummy-b = "*"\n')
    verify_cli_command([pixi, "install", "--locked", *args], ExitCode.FAILURE)
    verify_cli_command([pixi, "update", "--dry-run", *args])
    assert lock.read_bytes() == before
    verify_cli_command([pixi, "install", *args])
    assert lock.read_bytes() != before
    assert list((tmp_pixi_workspace / ".pixi/envs/default/conda-meta").glob("dummy-b-*.json"))
    verify_cli_command([pixi, "remove", *args, "dummy-b"])
    assert not list((tmp_pixi_workspace / ".pixi/envs/default/conda-meta").glob("dummy-b-*.json"))
    verify_cli_command([pixi, "add", *args, "dummy-b"])
    assert "platforms = []" in manifest.read_text()
    assert not (tmp_pixi_workspace / "pixi.lock").exists()

    # Removing the disposable cache must recover normally.
    lock.unlink()
    verify_cli_command([pixi, "install", *args])
    assert lock.exists()


def test_mixed_local_and_shared_locks(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace / "pixi.toml"
    manifest.write_text(f'''[workspace]
name = "mixed"
channels = ["{dummy_channel_1}"]
platforms = ["{CURRENT_PLATFORM}"]
[dependencies]
dummy-a = "*"
[environments.local]
platforms = []
dependencies = {{ dummy-b = "*" }}
''')
    args = ["--manifest-path", manifest]
    verify_cli_command([pixi, "install", "--all", *args])
    shared = tmp_pixi_workspace / "pixi.lock"
    local = tmp_pixi_workspace / ".pixi/locks/local" / f"{CURRENT_PLATFORM}.lock"
    assert set(yaml.safe_load(shared.read_text())["environments"]) == {"default"}
    assert set(yaml.safe_load(local.read_text())["environments"]) == {"local"}
    assert "dummy-b" not in shared.read_text()
    assert "dummy-b" in local.read_text()
    verify_cli_command([pixi, "install", "--all", "--locked", *args])

    # Transition to entirely local storage, including an already-valid local cache.
    manifest.write_text(
        manifest.read_text().replace(f'platforms = ["{CURRENT_PLATFORM}"]', "platforms = []")
    )
    verify_cli_command([pixi, "install", "--all", *args])
    assert not shared.exists()
    assert (tmp_pixi_workspace / ".pixi/locks/default" / f"{CURRENT_PLATFORM}.lock").exists()

    # Returning to regular mode produces a portable shared lock again.
    manifest.write_text(
        manifest.read_text().replace("platforms = []", f'platforms = ["{CURRENT_PLATFORM}"]')
    )
    verify_cli_command([pixi, "install", "--all", *args])
    assert set(yaml.safe_load(shared.read_text())["environments"]) == {"default", "local"}


def test_local_feature_platform_restrictions(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace / "pixi.toml"
    other = "win-64" if CURRENT_PLATFORM != "win-64" else "linux-64"
    source = f'''[workspace]
name = "restricted"
channels = ["{dummy_channel_1}"]
platforms = ["{CURRENT_PLATFORM}", "{other}"]
[feature.restricted]
platforms = ["{other}"]
dependencies = {{ dummy-b = "*" }}
[environments.local]
platforms = []
features = ["restricted"]
dependencies = {{ dummy-a = "*" }}
'''
    manifest.write_text(source)
    args = ["--manifest-path", manifest]
    for command in (["install", "-e", "local"], ["lock"]):
        verify_cli_command(
            [pixi, *command, *args],
            ExitCode.FAILURE,
            stderr_contains="feature platform restrictions",
        )
    assert not (tmp_pixi_workspace / "pixi.lock").exists()

    # The empty opt-in must not suppress the feature carrying it, either.
    manifest.write_text(
        source.replace(f'platforms = ["{other}"]', f'platforms = ["{CURRENT_PLATFORM}"]')
    )
    verify_cli_command([pixi, "install", "-e", "local", *args])
    metadata = tmp_pixi_workspace / ".pixi/envs/local/conda-meta"
    assert list(metadata.glob("dummy-a-*.json"))
    assert list(metadata.glob("dummy-b-*.json"))


def test_local_locks_require_explicit_legacy_upgrade(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace / "pixi.toml"
    manifest.write_text(f'''[workspace]
name = "legacy"
channels = ["{dummy_channel_1}"]
platforms = ["{CURRENT_PLATFORM}"]
[environments.local]
platforms = []
dependencies = {{ dummy-a = "*" }}
''')
    shared = tmp_pixi_workspace / "pixi.lock"
    shared.write_text(
        yaml.safe_dump(
            {
                "version": 6,
                "environments": {
                    "default": {
                        "channels": [{"url": dummy_channel_1}],
                        "packages": {CURRENT_PLATFORM: []},
                    }
                },
                "packages": [],
            }
        )
    )
    before = shared.read_bytes()
    args = ["--manifest-path", manifest]
    # add/update bypass Workspace::update_lock_file and must also be guarded.
    for command in (
        ["install", "-e", "local"],
        ["update", "--no-install"],
        ["add", "--no-install", "--environment", "local", "dummy-b"],
    ):
        verify_cli_command(
            [pixi, *command, *args],
            ExitCode.FAILURE,
            stderr_contains="pixi lock",
        )
        assert shared.read_bytes() == before
        assert not (tmp_pixi_workspace / ".pixi/locks/local" / f"{CURRENT_PLATFORM}.lock").exists()
    verify_cli_command(
        [pixi, "lock", "--dry-run", *args], stderr_contains="re-solving all environments"
    )
    assert shared.read_bytes() == before
    verify_cli_command([pixi, "lock", *args])
    assert yaml.safe_load(shared.read_text())["version"] == 7
    verify_cli_command([pixi, "install", "--all", "--locked", *args])

    # Legacy metadata can originate in a local cache, not just the shared lock.
    local = tmp_pixi_workspace / ".pixi/locks/local" / f"{CURRENT_PLATFORM}.lock"
    local.write_text(
        yaml.safe_dump(
            {
                "version": 6,
                "environments": {
                    "local": {
                        "channels": [{"url": dummy_channel_1}],
                        "packages": {CURRENT_PLATFORM: []},
                    }
                },
                "packages": [],
            }
        )
    )
    before = shared.read_bytes(), local.read_bytes()
    verify_cli_command([pixi, "install", *args], ExitCode.FAILURE, stderr_contains="pixi lock")
    assert (shared.read_bytes(), local.read_bytes()) == before
    verify_cli_command([pixi, "lock", *args], stderr_contains="re-solving all environments")
    assert yaml.safe_load(local.read_text())["version"] == 7


def test_local_solve_group_is_rejected(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace / "pixi.toml"
    manifest.write_text(f'''[workspace]
name = "local"
channels = ["{dummy_channel_1}"]
platforms = []
[environments.default]
solve-group = "shared"
''')
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
        ExitCode.FAILURE,
        stderr_contains="cannot belong to a solve group",
    )
