"""Solving and cache semantics of `--offline`.

In offline mode a solve may only pick packages that can be installed without
touching the network. The restriction is only observable against records whose
URL scheme is not `file`, since local channels are carved out unconditionally.
These tests serve the dummy channels over plain HTTP so records carry `http://`
URLs, which also makes it possible to take the channel away mid-test and check
that a solve really did promise a network-free install.

Offline mode also puts the repodata gateway in cache-only mode, so a test that
wants to reach the solve at all has to warm the repodata cache for the exact
channel URL first. `serving` binds a fresh port per context, so that warming
has to happen inside the same `with` block.
"""

import functools
import http.server
import json
import shutil
import socketserver
import threading
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path

import pytest

from .common import CURRENT_PLATFORM, ExitCode, exec_extension, verify_cli_command

CHANNELS = Path(__file__).parents[1].joinpath("data", "channels", "channels")


class _QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, format: str, *args: object) -> None:
        pass


@contextmanager
def serving(directory: Path) -> Iterator[str]:
    """Serve `directory` over HTTP, shutting the server down on exit.

    A context manager rather than a fixture because several tests need to stop
    the server while the test is still running.
    """
    handler = functools.partial(_QuietHandler, directory=str(directory.resolve()))
    # Port 0 lets the OS pick a free port, so parallel xdist workers don't collide.
    server = socketserver.ThreadingTCPServer(("127.0.0.1", 0), handler)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_address[1]}"
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


@pytest.fixture(scope="module")
def http_channel() -> Iterator[str]:
    with serving(CHANNELS / "dummy_channel_1") as url:
        yield url


def write_manifest(
    workspace: Path,
    channel: str,
    dependency: str = 'dummy-a = "*"',
    platforms: str = f'["{CURRENT_PLATFORM}"]',
    extra: str = "",
) -> Path:
    manifest = workspace / "pixi.toml"
    manifest.write_text(
        f"""
[workspace]
name = "offline-test"
channels = ["{channel}"]
platforms = {platforms}

[dependencies]
{dependency}
{extra}
"""
    )
    return manifest


def cache_env(cache: Path) -> dict[str, str]:
    cache.mkdir(parents=True, exist_ok=True)
    return {"PIXI_CACHE_DIR": str(cache)}


def package_cache_dir(cache: Path) -> Path:
    return cache / "pkgs"


def warm_repodata(pixi: Path, manifest: Path, env: dict[str, str]) -> None:
    """Solve once online so a later `--offline` run fails on the restriction
    rather than on repodata that is not in the cache.

    `pixi lock` fills the repodata cache but leaves the package cache alone, so
    the restriction still has something to bite on.
    """
    verify_cli_command([pixi, "lock", "--manifest-path", manifest], env=env)
    (manifest.parent / "pixi.lock").unlink()


@pytest.fixture(scope="module")
def pixi_binary(request: pytest.FixtureRequest) -> Path:
    """Module-scoped twin of the function-scoped `pixi` fixture, needed by the
    module-scoped warm cache below."""
    build = request.config.getoption("--pixi-build")
    return Path(exec_extension(str(Path(__file__).parent / f"../../target/pixi/{build}/pixi")))


@pytest.fixture(scope="module")
def warm_cache(
    pixi_binary: Path, tmp_path_factory: pytest.TempPathFactory, http_channel: str
) -> Path:
    """A cache with repodata for the HTTP channel and `dummy-a`, `dummy-c`,
    `dummy-g` and `dummy-b` extracted into the package cache.

    Module scoped because warming it costs a real install; tests that need to
    mutate it copy it first with `copy_cache`.
    """
    cache = tmp_path_factory.mktemp("warm-cache")
    workspace = tmp_path_factory.mktemp("warm-workspace")
    manifest = write_manifest(workspace, http_channel, dependency='dummy-a = "*"\ndummy-g = "*"')
    verify_cli_command([pixi_binary, "install", "--manifest-path", manifest], env=cache_env(cache))
    return cache


def copy_cache(warm: Path, tmp_path: Path) -> Path:
    """A private copy of the warm cache, so a test may delete entries from it."""
    cache = tmp_path / "cache"
    shutil.copytree(warm, cache)
    return cache


def drop_from_package_cache(cache: Path, package: str) -> None:
    """Remove every package cache entry whose directory names `package`."""
    pkgs = package_cache_dir(cache)
    removed = False
    for entry in pkgs.iterdir():
        if entry.is_dir() and entry.name.startswith(f"{package}-"):
            shutil.rmtree(entry)
            removed = True
    assert removed, (
        f"expected {package} in the package cache: {sorted(p.name for p in pkgs.iterdir())}"
    )


def mutated_channel(source: Path, destination: Path, package_prefix: str, sha256: str) -> Path:
    """A copy of `source` whose repodata advertises a different sha256 for the
    package named by `package_prefix`.

    This is what a channel serving *different content* under the same name,
    version and build string looks like from the solver's point of view: the
    package cache keys on name-version-build only, so the recorded hash is the
    only thing that can tell the two apart.
    """
    shutil.copytree(source, destination)
    for repodata in destination.rglob("repodata.json"):
        data = json.loads(repodata.read_text())
        for group in ("packages", "packages.conda"):
            for filename, record in data.get(group, {}).items():
                if filename.startswith(f"{package_prefix}-"):
                    record["sha256"] = sha256
        repodata.write_text(json.dumps(data))
    return destination


# --- the restriction itself -------------------------------------------------


def test_remote_records_are_excluded_with_a_reason(
    pixi: Path, tmp_pixi_workspace: Path, tmp_path: Path, http_channel: str
) -> None:
    """The failure must name the restriction, not report a bare missing package."""
    env = cache_env(tmp_path / "cache")
    manifest = write_manifest(tmp_pixi_workspace, http_channel)
    warm_repodata(pixi, manifest, env)

    output = verify_cli_command(
        [pixi, "lock", "--manifest-path", manifest, "--offline"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains="not available locally",
    )
    # A bare "no candidates" would hide *why* the package was unusable.
    assert "excluded because not available locally" in output.stderr


def test_local_channel_is_exempt_from_the_restriction(
    pixi: Path, tmp_pixi_workspace: Path, tmp_path: Path, dummy_channel_1: str
) -> None:
    """`file://` records need no download, so offline mode must not exclude them."""
    manifest = write_manifest(tmp_pixi_workspace, dummy_channel_1)

    verify_cli_command(
        [pixi, "lock", "--manifest-path", manifest, "--offline"],
        env=cache_env(tmp_path / "cache"),
        stderr_contains="dummy-a",
    )
    assert (tmp_pixi_workspace / "pixi.lock").is_file()


def test_unrestricted_solve_against_the_same_channel_succeeds(
    pixi: Path, tmp_pixi_workspace: Path, tmp_path: Path, http_channel: str
) -> None:
    """Control: the HTTP channel is solvable, so the failure above is the mode."""
    manifest = write_manifest(tmp_pixi_workspace, http_channel)

    verify_cli_command(
        [pixi, "lock", "--manifest-path", manifest],
        env=cache_env(tmp_path / "cache"),
        stderr_contains="dummy-a",
    )


# --- the sha256 check -------------------------------------------------------


def test_cached_package_with_a_different_sha256_is_not_locally_available(
    pixi: Path, tmp_pixi_workspace: Path, tmp_path: Path, warm_cache: Path
) -> None:
    """The cache directory name is only name-version-build, so a rebuilt
    package collides with the cached one. Offline mode must trust the sha256, not
    the directory name, or it promises an install that still needs the network."""
    cache = copy_cache(warm_cache, tmp_path)
    channel = mutated_channel(
        CHANNELS / "dummy_channel_1",
        tmp_path / "mutated",
        "dummy-a",
        "0" * 64,
    )

    with serving(channel) as url:
        manifest = write_manifest(tmp_pixi_workspace, url)
        warm_repodata(pixi, manifest, cache_env(cache))
        verify_cli_command(
            [pixi, "lock", "--manifest-path", manifest, "--offline"],
            ExitCode.FAILURE,
            env=cache_env(cache),
            stderr_contains="not available locally",
        )


def test_cached_package_with_a_matching_sha256_is_locally_available(
    pixi: Path, tmp_pixi_workspace: Path, tmp_path: Path, warm_cache: Path
) -> None:
    """Control for the test above: unmutated repodata resolves fine."""
    cache = copy_cache(warm_cache, tmp_path)
    channel = mutated_channel(
        CHANNELS / "dummy_channel_1",
        tmp_path / "unmutated",
        "does-not-exist",
        "0" * 64,
    )

    with serving(channel) as url:
        manifest = write_manifest(tmp_pixi_workspace, url)
        warm_repodata(pixi, manifest, cache_env(cache))
        verify_cli_command(
            [pixi, "lock", "--manifest-path", manifest, "--offline"],
            env=cache_env(cache),
        )


# --- transitive dependencies ------------------------------------------------


def test_missing_transitive_dependency_fails_the_solve(
    pixi: Path, tmp_pixi_workspace: Path, tmp_path: Path, http_channel: str, warm_cache: Path
) -> None:
    """`dummy-g` depends on `dummy-b`. With only the dependency evicted the
    solve must fail rather than lock a package whose dependency still needs a
    download.

    The copied warm cache already holds the repodata for `http_channel`, so
    offline mode reaches the solve.
    """
    cache = copy_cache(warm_cache, tmp_path)
    drop_from_package_cache(cache, "dummy-b")

    manifest = write_manifest(tmp_pixi_workspace, http_channel, dependency='dummy-g = "*"')
    output = verify_cli_command(
        [pixi, "lock", "--manifest-path", manifest, "--offline"],
        ExitCode.FAILURE,
        env=cache_env(cache),
        stderr_contains="not available locally",
    )
    assert "dummy-b" in output.stderr, (
        f"the report should name the dependency that is missing:\n{output.stderr}"
    )


def test_direct_dependency_present_but_transitive_missing_does_not_lock(
    pixi: Path, tmp_pixi_workspace: Path, tmp_path: Path, http_channel: str, warm_cache: Path
) -> None:
    """The failure above must not leave a lock file behind."""
    cache = copy_cache(warm_cache, tmp_path)
    drop_from_package_cache(cache, "dummy-c")

    manifest = write_manifest(tmp_pixi_workspace, http_channel)
    verify_cli_command(
        [pixi, "lock", "--manifest-path", manifest, "--offline"],
        ExitCode.FAILURE,
        env=cache_env(cache),
    )
    assert not (tmp_pixi_workspace / "pixi.lock").exists()


# --- the guarantee: an offline solve installs without network ----------------


def test_install_after_an_offline_solve_needs_no_network(
    pixi: Path, tmp_pixi_workspace: Path, tmp_path: Path, warm_cache: Path
) -> None:
    """The point of the restriction. Solve against a live channel, take the
    channel away, then install: everything the solve picked was already local,
    so this must succeed."""
    cache = copy_cache(warm_cache, tmp_path)
    channel = tmp_path / "channel"
    shutil.copytree(CHANNELS / "dummy_channel_1", channel)

    with serving(channel) as url:
        manifest = write_manifest(tmp_pixi_workspace, url)
        warm_repodata(pixi, manifest, cache_env(cache))
        verify_cli_command(
            [pixi, "lock", "--manifest-path", manifest, "--offline"],
            env=cache_env(cache),
        )

    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest, "--frozen"],
        env=cache_env(cache),
    )


# --- lock file records ------------------------------------------------------


@pytest.fixture
def committed_lock(pixi: Path, tmp_pixi_workspace: Path, tmp_path: Path) -> Iterator[Path]:
    """A workspace with a `pixi.lock` produced elsewhere, as a colleague would
    have committed it, and a channel that is still up. Yields the manifest.

    The author's cache is separate, so the workspace's own cache stays empty.
    """
    channel = tmp_path / "channel"
    shutil.copytree(CHANNELS / "dummy_channel_1", channel)
    with serving(channel) as url:
        manifest = write_manifest(tmp_pixi_workspace, url)
        verify_cli_command(
            [pixi, "lock", "--manifest-path", manifest],
            env=cache_env(tmp_path / "author-cache"),
        )
        yield manifest


def test_offline_does_restrict_installs_from_a_satisfied_lock(
    pixi: Path, tmp_path: Path, committed_lock: Path
) -> None:
    """A lock file that already satisfies the manifest means there is no solve
    left to restrict, but `--offline` still acts at the transport layer, so the
    install cannot fetch what the lock names."""
    verify_cli_command(
        [pixi, "install", "--manifest-path", committed_lock, "--frozen", "--offline"],
        ExitCode.FAILURE,
        env=cache_env(tmp_path / "cold-offline"),
        stderr_contains="offline",
    )


def test_a_damaged_cache_entry_is_still_treated_as_available(
    pixi: Path, tmp_pixi_workspace: Path, tmp_path: Path, warm_cache: Path
) -> None:
    """Known limitation: the cache index checks that an entry exists and that
    its recorded sha256 matches, not that its contents are intact. An entry
    left behind by an interrupted extraction still counts as available, so the
    solve succeeds and the install re-fetches.

    Documented on `CacheIndex` in rattler; pinned here so a change to that
    contract is noticed on the pixi side.
    """
    cache = copy_cache(warm_cache, tmp_path)
    for entry in package_cache_dir(cache).iterdir():
        if entry.is_dir():
            for child in entry.iterdir():
                shutil.rmtree(child) if child.is_dir() else child.unlink()

    channel = tmp_path / "channel"
    shutil.copytree(CHANNELS / "dummy_channel_1", channel)
    with serving(channel) as url:
        manifest = write_manifest(tmp_pixi_workspace, url)
        warm_repodata(pixi, manifest, cache_env(cache))
        verify_cli_command(
            [pixi, "lock", "--manifest-path", manifest, "--offline"],
            env=cache_env(cache),
        )
