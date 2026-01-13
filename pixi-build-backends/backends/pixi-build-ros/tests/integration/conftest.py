"""Pytest fixtures for pixi-build-ros integration tests."""

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
            f"Local backend channel not found at {channel_dir}. "
            "Run 'pixi run create-channel' to generate it."
        )
    return channel_dir.as_uri()


@pytest.fixture
def build_data() -> Path:
    """Return the integration test data directory."""
    return Path(__file__).parent / "data"


@pytest.fixture
def pixi() -> Path:
    """Return path to the pixi executable."""
    # Use the pixi from target/pixi/release
    pixi_bin = repo_root() / "target" / "pixi" / "release" / exec_extension("pixi")
    if not pixi_bin.is_file():
        raise RuntimeError(
            f"pixi binary not found at {pixi_bin}. "
            "This is a bug with the test setup: make sure the task depends on 'build-release'."
        )
    return pixi_bin


@pytest.fixture
def tmp_pixi_workspace(tmp_path: Path) -> Path:
    """Create a temporary workspace for tests."""
    return tmp_path
