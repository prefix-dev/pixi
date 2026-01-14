"""Pytest fixtures for pixi-build-ros integration tests."""

import shutil
import sys
import tempfile
from collections.abc import Iterator
from pathlib import Path

import pytest

from .common import exec_extension


def repo_root() -> Path:
    """Return the root of the pixi repository."""
    # integration/ -> tests/ -> pixi-build-ros/ -> backends/ -> pixi-build-backends/ -> pixi/
    return Path(__file__).parents[5]


@pytest.fixture(scope="session")
def local_backend_channel_uri() -> str:
    """Return file URI to the local ROS backend channel."""
    channel_dir = Path(__file__).parent / "artifacts-channel"
    if not channel_dir.is_dir() or not any(channel_dir.rglob("repodata.json")):
        raise RuntimeError(
            f"Local backend channel not found at {channel_dir}. Run 'pixi run create-channel' to generate it."
        )
    return channel_dir.as_uri()


@pytest.fixture
def build_data() -> Path:
    """Return the integration test data directory."""
    return Path(__file__).parent / "data"


@pytest.fixture
def pixi() -> Path:
    """Return path to the pixi executable.

    Locally, use the built binary in target/pixi/release.
    In CI, the pre-built binary is downloaded to target/pixi/release.
    """
    pixi_bin = repo_root() / "target" / "pixi" / "release" / exec_extension("pixi")
    if not pixi_bin.is_file():
        raise RuntimeError(f"pixi binary not found at {pixi_bin}. ")
    return pixi_bin


@pytest.fixture
def tmp_pixi_workspace(tmp_path: Path) -> Iterator[Path]:
    """Create a temporary workspace for tests.

    On Windows, uses a shorter path to avoid MAX_PATH (260 char) limitations.
    The build process creates deeply nested paths that can exceed this limit.
    """
    if sys.platform == "win32":
        # Use a short base path on Windows to avoid MAX_PATH issues
        short_base = Path(tempfile.gettempdir()) / "pxros"
        short_base.mkdir(parents=True, exist_ok=True)
        workspace = Path(tempfile.mkdtemp(dir=short_base))
        try:
            yield workspace
        finally:
            shutil.rmtree(workspace, ignore_errors=True)
    else:
        yield tmp_path
