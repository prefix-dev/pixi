"""Command coverage for `--offline`.

`test_offline_solving.py` pins the restriction itself. This file asks a
different question: does every command that accepts `--offline` actually route
it to the solve, and do the commands that build on top of a restricted solve
behave the way they do online?

Like `test_offline_solving.py`, everything observable happens against an
HTTP-served channel, because `file://` records are exempt from the restriction.
Offline mode also reads repodata from the cache only, so each test warms the
cache for its channel URL first.
"""

from collections.abc import Iterator
from pathlib import Path

import pytest

from .common import CURRENT_PLATFORM, ExitCode, verify_cli_command
from .test_offline_solving import CHANNELS, cache_env, serving, warm_repodata, write_manifest

# The reason the solver prints for a record ruled out by offline mode.
EXCLUDED = "excluded because not available locally"


@pytest.fixture(scope="module")
def http_channels() -> Iterator[str]:
    """The whole `channels/` tree over HTTP, so a single server backs both
    `dummy_channel_1` and `multiple_versions_channel_1`."""
    with serving(CHANNELS) as url:
        yield url


@pytest.fixture
def dummy_http(http_channels: str) -> str:
    return f"{http_channels}/dummy_channel_1"


@pytest.fixture
def versions_http(http_channels: str) -> str:
    return f"{http_channels}/multiple_versions_channel_1"


def isolated_cache(tmp_path: Path) -> dict[str, str]:
    """A private package/repodata cache, so one test can't warm another's."""
    return cache_env(tmp_path / "offline-cache")


def isolated_home(tmp_path: Path) -> dict[str, str]:
    """Point every global-config search path at a private directory.

    Without this a test would read the developer's own `~/.pixi/config.toml`
    and `$XDG_CONFIG_HOME/pixi/config.toml`, and `pixi global` would install
    into their real global environment.
    """
    home = tmp_path / "offline-home"
    home.mkdir(exist_ok=True)
    xdg = tmp_path / "offline-xdg"
    xdg.mkdir(exist_ok=True)
    return {
        "PIXI_HOME": str(home),
        "XDG_CONFIG_HOME": str(xdg),
        "APPDATA": str(xdg),
    }


def write_global_manifest(env: dict[str, str], channel: str) -> Path:
    manifests = Path(env["PIXI_HOME"]) / "manifests"
    manifests.mkdir(exist_ok=True)
    manifest = manifests / "pixi-global.toml"
    manifest.write_text(
        f"""
version = 1

[envs.dummy]
channels = ["{channel}"]
dependencies = {{ dummy-a = "*" }}
exposed = {{ dummy-a = "dummy-a" }}
"""
    )
    return manifest


def warm_channel_repodata(pixi: Path, channel: str, directory: Path, env: dict[str, str]) -> None:
    """Warm the repodata cache for `channel` from a throwaway workspace.

    Commands without a manifest of their own - `pixi exec`, `pixi global` - hit
    the same repodata cache, which is keyed by channel URL.
    """
    directory.mkdir(parents=True, exist_ok=True)
    warm_repodata(pixi, write_manifest(directory, channel), env)


def warm_package_cache(
    pixi: Path,
    channel: str,
    directory: Path,
    env: dict[str, str],
    dependency: str = 'dummy-a = "*"',
) -> None:
    """Install once online, so both the repodata and the packages are cached
    and a later offline solve has something to resolve to."""
    directory.mkdir(parents=True, exist_ok=True)
    manifest = write_manifest(directory, channel, dependency=dependency)
    verify_cli_command([pixi, "install", "--manifest-path", manifest], env=env)


# --- does offline mode reach every command that accepts it? -----------------


@pytest.mark.parametrize("command", ["install", "shell-hook", "reinstall", "lock", "update"])
def test_workspace_commands_restrict_the_solve(
    pixi: Path, tmp_pixi_workspace: Path, tmp_path: Path, dummy_http: str, command: str
) -> None:
    manifest = write_manifest(tmp_pixi_workspace, dummy_http)
    env = isolated_cache(tmp_path)
    warm_repodata(pixi, manifest, env)

    verify_cli_command(
        [pixi, *command.split(), "--manifest-path", manifest, "--offline"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains=EXCLUDED,
    )


def test_run_restricts_the_solve(
    pixi: Path, tmp_pixi_workspace: Path, tmp_path: Path, dummy_http: str
) -> None:
    manifest = write_manifest(tmp_pixi_workspace, dummy_http)
    manifest.write_text(manifest.read_text() + '\n[tasks]\nhello = "echo hi"\n')
    env = isolated_cache(tmp_path)
    warm_repodata(pixi, manifest, env)

    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "--offline", "hello"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains=EXCLUDED,
    )


# --- pixi exec: its own solve, outside the command dispatcher ---------------


def test_exec_restricts_the_solve(pixi: Path, tmp_path: Path, dummy_http: str) -> None:
    env = isolated_cache(tmp_path) | isolated_home(tmp_path)
    warm_channel_repodata(pixi, dummy_http, tmp_path / "warm", env)

    verify_cli_command(
        [pixi, "exec", "--offline", "--channel", dummy_http, "--spec", "dummy-a", "dummy-a"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains=EXCLUDED,
    )


def exec_prefix_versions(cache: Path) -> list[str]:
    """The package versions installed into `pixi exec`'s cached prefixes."""
    return sorted(
        p.name for p in cache.glob("cached-envs-v0/*/conda-meta/*.json") if p.name != "history"
    )


def test_exec_does_not_reuse_a_restricted_prefix_for_an_unrestricted_run(
    pixi: Path, tmp_path: Path, versions_http: str
) -> None:
    """`pixi exec` prefixes are content-addressed and shared across processes.
    An offline solve can resolve an older version than an online one, so the two
    must not land on the same prefix - otherwise the restricted result is
    silently served to every later online `pixi exec`."""
    env = isolated_cache(tmp_path) | isolated_home(tmp_path)
    cache = Path(env["PIXI_CACHE_DIR"])

    # Warm the package cache with 0.1.0 only, so an offline solve can pick
    # 0.1.0 while an online one would pick 0.2.0.
    warm_package_cache(
        pixi, versions_http, tmp_path / "warmup", env, dependency='package = "==0.1.0"'
    )

    verify_cli_command(
        [
            pixi,
            "exec",
            "--offline",
            "--channel",
            versions_http,
            "--spec",
            "package",
            "offline-no-such-command",
        ],
        ExitCode.FAILURE,
        env=env,
        stderr_contains="failed to execute",
    )
    verify_cli_command(
        [
            pixi,
            "exec",
            "--channel",
            versions_http,
            "--spec",
            "package",
            "offline-no-such-command",
        ],
        ExitCode.FAILURE,
        env=env,
        stderr_contains="failed to execute",
    )

    installed = exec_prefix_versions(cache)
    assert any("0.2.0" in name for name in installed), (
        "an online `pixi exec` must not reuse the prefix an `--offline` run "
        f"built from an older package; found {installed}"
    )


# --- pixi global: a separate project type with its own dispatcher -----------


def test_global_install_restricts_the_solve(pixi: Path, tmp_path: Path, dummy_http: str) -> None:
    env = isolated_cache(tmp_path) | isolated_home(tmp_path)
    warm_channel_repodata(pixi, dummy_http, tmp_path / "warm", env)

    verify_cli_command(
        [pixi, "global", "install", "--offline", "--channel", dummy_http, "dummy-a"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains=EXCLUDED,
    )


def test_global_sync_restricts_the_solve(pixi: Path, tmp_path: Path, dummy_http: str) -> None:
    env = isolated_cache(tmp_path) | isolated_home(tmp_path)
    warm_channel_repodata(pixi, dummy_http, tmp_path / "warm", env)
    write_global_manifest(env, dummy_http)

    verify_cli_command(
        [pixi, "global", "sync", "--offline"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains=EXCLUDED,
    )


# --- commands that accept the flag but have no solve to restrict ------------


def test_search_reports_packages_from_the_repodata_cache(
    pixi: Path, tmp_path: Path, dummy_http: str
) -> None:
    """`pixi search` only queries repodata, so the restriction on the solve
    changes nothing. Pinned so a future change is a deliberate one.

    Without a workspace in scope `search` would query every known platform, and
    offline mode only has the current one cached, so pin the platform.
    """
    env = isolated_cache(tmp_path) | isolated_home(tmp_path)
    warm_channel_repodata(pixi, dummy_http, tmp_path / "warm", env)

    verify_cli_command(
        [
            pixi,
            "search",
            "--offline",
            "--platform",
            CURRENT_PLATFORM,
            "--channel",
            dummy_http,
            "dummy-a",
        ],
        env=env,
        stdout_contains="dummy-a",
    )


# --- commands built on top of a restricted solve ----------------------------


def test_upgrade_works_offline(
    pixi: Path, tmp_pixi_workspace: Path, tmp_path: Path, dummy_http: str
) -> None:
    """`upgrade` is not special-cased: it runs and rewrites the manifest exactly
    as it does online, from whatever the restricted solve resolved."""
    env = isolated_cache(tmp_path)
    warm_package_cache(pixi, dummy_http, tmp_path / "warmup", env)

    manifest = write_manifest(tmp_pixi_workspace, dummy_http)
    verify_cli_command(
        [pixi, "upgrade", "--manifest-path", manifest, "--offline"],
        env=env,
    )
    assert 'dummy-a = ">=0.1.0,<0.2"' in manifest.read_text()


def test_upgrade_works_with_a_local_channel(
    pixi: Path, tmp_pixi_workspace: Path, tmp_path: Path, dummy_channel_1: str
) -> None:
    manifest = write_manifest(tmp_pixi_workspace, dummy_channel_1)

    verify_cli_command(
        [pixi, "upgrade", "--manifest-path", manifest, "--offline"],
        env=isolated_cache(tmp_path),
    )
    assert 'dummy-a = ">=0.1.0,<0.2"' in manifest.read_text()


def test_upgrade_dry_run_works(
    pixi: Path, tmp_pixi_workspace: Path, tmp_path: Path, dummy_http: str
) -> None:
    env = isolated_cache(tmp_path)
    warm_package_cache(pixi, dummy_http, tmp_path / "warmup", env)

    manifest = write_manifest(tmp_pixi_workspace, dummy_http)
    before = manifest.read_text()

    verify_cli_command(
        [pixi, "upgrade", "--manifest-path", manifest, "--offline", "--dry-run"],
        env=env,
    )
    assert manifest.read_text() == before, "a dry run must not touch the manifest"


def test_add_pins_from_the_restricted_solve(
    pixi: Path, tmp_pixi_workspace: Path, tmp_path: Path, versions_http: str
) -> None:
    """`add` is not special-cased either. The channel offers 0.2.0 but only
    0.1.0 is cached, so the bound recorded describes what was available
    locally. That is a deliberate choice, so pin it."""
    env = isolated_cache(tmp_path)
    warm_package_cache(
        pixi, versions_http, tmp_path / "warmup", env, dependency='package = "==0.1.0"'
    )

    manifest = write_manifest(tmp_pixi_workspace, versions_http, dependency="")
    verify_cli_command(
        [pixi, "add", "--manifest-path", manifest, "--offline", "--no-install", "package"],
        env=env,
    )

    assert ">=0.1.0,<0.2" in manifest.read_text(), (
        "`pixi add --offline` should record a bound from the version it "
        f"resolved locally:\n{manifest.read_text()}"
    )


# --- the lock file a restricted solve leaves behind --------------------------


def test_lock_file_warning_is_emitted_for_a_restricted_solve(
    pixi: Path, tmp_pixi_workspace: Path, tmp_path: Path, dummy_channel_1: str
) -> None:
    manifest = write_manifest(tmp_pixi_workspace, dummy_channel_1)

    verify_cli_command(
        [pixi, "lock", "--manifest-path", manifest, "--offline"],
        env=isolated_cache(tmp_path),
        stderr_contains="may pin older versions",
    )


def test_lock_file_warning_is_absent_without_the_flag(
    pixi: Path, tmp_pixi_workspace: Path, tmp_path: Path, dummy_channel_1: str
) -> None:
    manifest = write_manifest(tmp_pixi_workspace, dummy_channel_1)

    verify_cli_command(
        [pixi, "lock", "--manifest-path", manifest],
        env=isolated_cache(tmp_path),
        stderr_excludes="may pin older versions",
    )


def test_update_warns_that_the_lock_file_may_be_stale(
    pixi: Path, tmp_pixi_workspace: Path, tmp_path: Path, dummy_channel_1: str
) -> None:
    manifest = write_manifest(tmp_pixi_workspace, dummy_channel_1)

    verify_cli_command(
        [pixi, "update", "--manifest-path", manifest, "--offline"],
        env=isolated_cache(tmp_path),
        stderr_contains=["may pin older versions", "pixi.lock"],
    )


def test_platform_specific_manifest_is_unaffected(
    pixi: Path, tmp_pixi_workspace: Path, tmp_path: Path, dummy_channel_1: str
) -> None:
    """A sanity check that the exemption for local channels survives a
    manifest that names the current platform explicitly."""
    manifest = tmp_pixi_workspace / "pixi.toml"
    manifest.write_text(
        f"""
[workspace]
name = "offline-platform"
channels = ["{dummy_channel_1}"]
platforms = ["{CURRENT_PLATFORM}"]

[target.{CURRENT_PLATFORM}.dependencies]
dummy-a = "*"
"""
    )

    verify_cli_command(
        [pixi, "lock", "--manifest-path", manifest, "--offline"],
        env=isolated_cache(tmp_path),
    )
    assert (tmp_pixi_workspace / "pixi.lock").is_file()
