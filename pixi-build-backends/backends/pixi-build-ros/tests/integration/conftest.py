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


@pytest.fixture
def build_data() -> Path:
    """Return the integration test data directory."""
    return Path(__file__).parent / "data"


@pytest.fixture
def pixi() -> Path:
    """Return path to the pixi executable.

    Locally, use the built binary in target/pixi/release.
    In CI, the pre-built binary is downloaded to target/pixi/release.
    If not found, use pixi from PATH.
    If neither is found, raise an error.
    """
    pixi_bin = repo_root() / "target" / "pixi" / "release" / exec_extension("pixi")
    on_path_pixi = shutil.which("pixi")

    if not pixi_bin.is_file() and not on_path_pixi:
        raise RuntimeError(f"pixi binary not found at {pixi_bin} or in PATH. Please build pixi first.")
    return pixi_bin


@pytest.fixture
def tmp_pixi_workspace(tmp_path: Path) -> Iterator[Path]:
    """Create a temporary workspace for tests.

    On Windows, uses a shorter path to avoid MAX_PATH (260 char) limitations.
    The build process creates deeply nested paths that can exceed this limit.
    """
    if sys.platform == "win32":
        # Use a very short base path on Windows to avoid MAX_PATH issues.
        # The standard temp directory (e.g. C:\Users\<user>\AppData\Local\Temp)
        # is already quite long, so we use C:\.r instead.
        short_base = Path("C:/.r")
        short_base.mkdir(parents=True, exist_ok=True)
        workspace = Path(tempfile.mkdtemp(dir=short_base))
        try:
            yield workspace
        finally:
            shutil.rmtree(workspace, ignore_errors=True)
    else:
        yield tmp_path
