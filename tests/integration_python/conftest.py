from collections.abc import Callable, Iterator
from pathlib import Path
from typing import Any
import os
import stat
import shutil
import sys
import tempfile
import time

import pytest

from .common import CONDA_FORGE_CHANNEL, exec_extension, repo_root


def pytest_addoption(parser: pytest.Parser) -> None:
    parser.addoption(
        "--pixi-build",
        action="store",
        default="release",
        help="Specify the pixi build type (e.g., release or debug)",
    )


def pytest_configure(config: pytest.Config) -> None:
    # Keep temp dirs inside the workspace so they share a filesystem with the
    # pixi cache (cross-FS rename/copy in /tmp is much slower). The fixed
    # `pytest-temp/` parent matches the literal path in `workspace.exclude`
    # in Cargo.toml (cargo doesn't glob-expand exclude entries).
    #
    # Rooting pytest's automatic numbered dirs here (instead of passing
    # --basetemp) keeps pytest's own basetemp management: concurrent runs get
    # distinct lock-protected `pytest-of-<user>/pytest-<N>` dirs, and dirs
    # from old runs are pruned down to `tmp_path_retention_count`, so
    # leftovers from failed or interrupted runs can't accumulate forever.
    # xdist workers get an explicit --basetemp below the controller's
    # numbered dir, so both the numbering and the pruning run only once.
    if not config.option.basetemp:
        parent = config.rootpath / "pytest-temp"
        parent.mkdir(exist_ok=True)
        os.environ["PYTEST_DEBUG_TEMPROOT"] = str(parent)


@pytest.fixture
def pixi(request: pytest.FixtureRequest) -> Path:
    pixi_build = request.config.getoption("--pixi-build")
    pixi_path = Path(__file__).parent.joinpath(f"../../target/pixi/{pixi_build}/pixi")
    return Path(exec_extension(str(pixi_path)))


@pytest.fixture(scope="session", autouse=True)
def setup_build_backend_override(request: pytest.FixtureRequest) -> None:
    """
    Sets up PIXI_BUILD_BACKEND_OVERRIDE for Rust backends.

    Points to binaries in target/pixi/{build_type}/ based on --pixi-build
    option. The workspace-built backends match the build backend API of the
    pixi binary under test, which released backends from the channels do not
    necessarily do.
    """
    build_type = request.config.getoption("--pixi-build")
    backends_bin_dir = repo_root() / "target" / "pixi" / build_type

    if not backends_bin_dir.is_dir():
        return  # Skip if not built yet

    backends = [
        "pixi-build-cmake",
        "pixi-build-python",
        "pixi-build-rattler-build",
        "pixi-build-ros",
        "pixi-build-rust",
    ]

    override_parts: list[str] = []
    missing_files: list[Path] = []
    for backend in backends:
        backend_path = backends_bin_dir / exec_extension(backend)
        if backend_path.is_file():
            override_parts.append(f"{backend}={backend_path}")
        else:
            missing_files.append(backend_path)

    if missing_files:
        missing_list = "\n  ".join(str(p) for p in missing_files)
        build_cmd = "build-debug" if build_type == "debug" else "build-release"
        raise RuntimeError(
            f"Missing backend binaries:\n  {missing_list}\n"
            + f"Run 'pixi run {build_cmd}' to build them."
        )

    os.environ["PIXI_BUILD_BACKEND_OVERRIDE"] = ",".join(override_parts)


@pytest.fixture(scope="session", autouse=True)
def isolated_pixi_cache_per_worker(
    tmp_path_factory: pytest.TempPathFactory, worker_id: str
) -> Iterator[None]:
    # Parallel xdist workers all share the user's `~/.cache/rattler/` by default,
    # which causes hash mismatches when two workers race to build the same source
    # package into the same `bld/` path. Give each worker its own cache root.
    # Single-process runs (worker_id == "master") keep the user's cache for speed.
    if worker_id == "master":
        yield
        return
    cache_dir = tmp_path_factory.mktemp("pixi-cache", numbered=False)
    os.environ["PIXI_CACHE_DIR"] = str(cache_dir)
    yield
    # The cache can hold hundreds of MB per worker and is not covered by
    # tmp_path_retention_policy (it's not a tmp_path fixture dir), so a run
    # with failures would retain it indefinitely. It's regenerable and useless
    # for debugging retained test workspaces: always remove it.
    shutil.rmtree(cache_dir, ignore_errors=True)


@pytest.fixture
def tmp_pixi_workspace(tmp_path: Path):
    """Create a temporary workspace for tests, with a .pixi config.

    On Windows, uses a shorter path to avoid MAX_PATH (260 char) limitations.
    The build process creates deeply nested paths that can exceed this limit.
    """

    pixi_config = f"""
# Reset to defaults
default-channels = ["{CONDA_FORGE_CHANNEL}"]
shell.change-ps1 = true
tls-no-verify = false
detached-environments = false
pinning-strategy = "semver"

[concurrency]
downloads = 50

[experimental]
use-environment-activation-cache = false

# Enable sharded repodata
[repodata-config."https://prefix.dev/"]
disable-sharded = false
"""

    if sys.platform == "win32":
        # Use a very short base path on Windows to avoid MAX_PATH issues.
        # The standard temp directory (e.g. C:\Users\<user>\AppData\Local\Temp)
        # is already quite long, so we use C:\.r instead.
        # Use no drive letter to avoid issues with different drives
        short_base = Path(r"\.r")
        short_base.mkdir(parents=True, exist_ok=True)
        workspace = Path(tempfile.mkdtemp(dir=short_base))
        workspace.joinpath(".pixi").mkdir()
        workspace.joinpath(".pixi/config.toml").write_text(pixi_config)

        def _robust_remove(func: Callable[..., Any], path: str, exc: BaseException) -> None:
            if isinstance(exc, PermissionError):
                if exc.winerror == 5:  # Access denied (read-only file)
                    os.chmod(path, stat.S_IWRITE)
                    func(path)
                    return
                if exc.winerror == 32:  # Sharing violation (file in use)
                    for _ in range(3):
                        time.sleep(0.5)
                        try:
                            os.chmod(path, stat.S_IWRITE)
                            func(path)
                            return
                        except PermissionError:
                            continue
            # If we get here, swallow the error (best-effort cleanup)

        try:
            yield workspace
        finally:
            shutil.rmtree(workspace, onexc=_robust_remove)
    else:
        tmp_path.joinpath(".pixi").mkdir()
        tmp_path.joinpath(".pixi/config.toml").write_text(pixi_config)
        yield tmp_path


@pytest.fixture
def test_data() -> Path:
    return Path(__file__).parents[1].joinpath("data").resolve()


@pytest.fixture
def pypi_data(test_data: Path) -> Path:
    """
    Returns the pixi pypi test data
    """
    return test_data.joinpath("pypi")


@pytest.fixture
def pixi_tomls(test_data: Path) -> Path:
    """
    Returns the pixi pypi test data
    """
    return test_data.joinpath("pixi_tomls")


@pytest.fixture
def mock_projects(test_data: Path) -> Path:
    return test_data.joinpath("mock-projects")


@pytest.fixture
def channels(test_data: Path) -> Path:
    return test_data.joinpath("channels", "channels")


@pytest.fixture
def dummy_channel_1(channels: Path) -> str:
    return channels.joinpath("dummy_channel_1").as_uri()


@pytest.fixture
def dummy_channel_2(channels: Path) -> str:
    return channels.joinpath("dummy_channel_2").as_uri()


@pytest.fixture
def multiple_versions_channel_1(channels: Path) -> str:
    return channels.joinpath("multiple_versions_channel_1").as_uri()


@pytest.fixture
def target_specific_channel_1(channels: Path) -> str:
    return channels.joinpath("target_specific_channel_1").as_uri()


@pytest.fixture
def non_self_expose_channel_1(channels: Path) -> str:
    return channels.joinpath("non_self_expose_channel_1").as_uri()


@pytest.fixture
def non_self_expose_channel_2(channels: Path) -> str:
    return channels.joinpath("non_self_expose_channel_2").as_uri()


@pytest.fixture
def virtual_packages_channel(channels: Path) -> str:
    return channels.joinpath("virtual_packages").as_uri()


@pytest.fixture
def shortcuts_channel_1(channels: Path) -> str:
    return channels.joinpath("shortcuts_channel_1").as_uri()


@pytest.fixture
def post_link_script_channel(channels: Path) -> str:
    return channels.joinpath("post_link_script_channel").as_uri()


@pytest.fixture
def deno_channel(channels: Path) -> str:
    return channels.joinpath("deno_channel").as_uri()


@pytest.fixture
def completions_channel_1(channels: Path) -> str:
    return channels.joinpath("completions_channel_1").as_uri()


@pytest.fixture
def doc_pixi_workspaces() -> Path:
    return Path(__file__).parents[2].joinpath("docs", "source_files", "pixi_workspaces")


@pytest.fixture
def external_commands_dir(tmp_path: Path) -> Path:
    """Create a temporary directory for external pixi commands"""
    commands_dir = tmp_path / "external_commands"
    commands_dir.mkdir()
    return commands_dir
