"""Real-network integration tests for `--offline` against conda-forge.

The rest of the offline suite runs against a local HTTP channel, which keeps it
fast but cannot exercise a *sharded* channel. conda-forge on prefix.dev serves a
shard index plus per-package shards, and the cache-only paths through those are
exactly what decides whether a restricted solve works. A locally served
`repodata.json` never reaches that code.

In cache-only mode a missing per-package shard counts as "no records" rather
than an error, so only a missing shard *index* fails outright. That is what lets
a partly warmed cache still produce a solve here.

These are marked `slow` so an upstream outage cannot block a pull request.
"""

import shutil
from pathlib import Path

import pytest

from .common import CURRENT_PLATFORM, verify_cli_command

# A deliberately old build. `tzdata` is noarch, has no dependencies and is a
# few kilobytes, so warming the cache with it costs almost nothing. The version
# only has to stay older than whatever conda-forge currently ships.
CACHED_VERSION = "2024a"


def write_manifest(workspace: Path, dependency: str) -> Path:
    manifest = workspace / "pixi.toml"
    manifest.write_text(
        f"""
[workspace]
name = "offline-conda-forge"
channels = ["conda-forge"]
platforms = ["{CURRENT_PLATFORM}"]

[dependencies]
{dependency}
"""
    )
    return manifest


def isolated_cache(tmp_path: Path) -> dict[str, str]:
    """A private cache, so the developer's warm cache cannot mask a failure."""
    cache = tmp_path / "conda-forge-cache"
    cache.mkdir(exist_ok=True)
    return {"PIXI_CACHE_DIR": str(cache)}


@pytest.mark.slow
def test_offline_resolves_the_cached_version(
    pixi: Path, tmp_pixi_workspace: Path, tmp_path: Path
) -> None:
    """The point of the feature: with the channel offering newer builds, a
    restricted solve must still pick the one that is already on disk.

    This is the test that fails if the exclusion map is not actually applied -
    an unrestricted solve resolves the newest `tzdata` conda-forge ships, not
    the cached one.
    """
    env = isolated_cache(tmp_path)

    # Warm the cache with exactly one build. This also fills the sharded
    # repodata cache, which `--offline` may only read from.
    warmup = tmp_path / "warmup"
    warmup.mkdir()
    warmup_manifest = write_manifest(warmup, f'tzdata = "=={CACHED_VERSION}"')
    verify_cli_command([pixi, "install", "--manifest-path", warmup_manifest], env=env)

    # Ask for *any* version. The cached shard still lists the newer builds;
    # only the restriction keeps the solve on the cached one.
    manifest = write_manifest(tmp_pixi_workspace, 'tzdata = "*"')
    verify_cli_command([pixi, "lock", "--manifest-path", manifest, "--offline"], env=env)

    lock_file = (tmp_pixi_workspace / "pixi.lock").read_text()
    assert f"tzdata-{CACHED_VERSION}" in lock_file, (
        f"`--offline` should resolve the cached tzdata {CACHED_VERSION}, "
        "but the lock file records something else. An unrestricted solve would "
        "pick the newest build conda-forge offers."
    )


@pytest.mark.slow
def test_pypi_workspace_relocks_offline_after_a_warm_install(
    pixi: Path, tmp_pixi_workspace: Path, tmp_path: Path
) -> None:
    """A workspace with PyPI dependencies must survive an offline relock once
    the caches are warm: the conda side is restricted, uv resolves from its
    own cache, and the conda-pypi mapping is served from its HTTP cache.

    The mapping is the regression here: its cache middleware has to sit
    outside the offline middleware, and serve cached entries regardless of
    freshness, or every offline solve of a PyPI workspace dies on the mapping
    fetch before solving anything.
    """
    env = isolated_cache(tmp_path)
    manifest = tmp_pixi_workspace / "pixi.toml"
    manifest.write_text(
        f"""
[workspace]
name = "offline-pypi"
channels = ["conda-forge"]
platforms = ["{CURRENT_PLATFORM}"]

[dependencies]
python = "3.13.*"

[pypi-dependencies]
six = "*"
"""
    )
    verify_cli_command([pixi, "install", "--manifest-path", manifest], env=env)
    (tmp_pixi_workspace / "pixi.lock").unlink()

    output = verify_cli_command(
        [pixi, "lock", "--manifest-path", manifest, "--offline"],
        env=env,
        stderr_contains="may pin older versions",
    )
    assert "PyPI dependencies" in output.stderr, (
        f"expected the PyPI warning alongside the lock warning:\n{output.stderr}"
    )


@pytest.mark.slow
def test_offline_install_after_warming_the_cache(
    pixi: Path, tmp_pixi_workspace: Path, tmp_path: Path
) -> None:
    """A full round trip against a sharded channel: once a solve has run
    online, the same solve must work with no network at all - shard index,
    per-package shards and package cache included.

    Unlike the test above this is a smoke test rather than a regression test:
    it is here to catch a future change that breaks the offline path outright,
    which the local-channel suite cannot see because it never exercises sharded
    repodata.
    """
    env = isolated_cache(tmp_path)
    manifest = write_manifest(tmp_pixi_workspace, 'tzdata = "*"')

    verify_cli_command([pixi, "install", "--manifest-path", manifest], env=env)

    # Drop the prefix so the second run has to re-install from the package
    # cache rather than reporting the environment as already up to date.
    shutil.rmtree(tmp_pixi_workspace / ".pixi" / "envs")

    verify_cli_command([pixi, "install", "--manifest-path", manifest, "--offline"], env=env)
