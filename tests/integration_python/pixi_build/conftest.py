"""Pytest fixtures for pixi-build integration tests."""

import shutil
from pathlib import Path

import pytest

from .common import CURRENT_PLATFORM, Workspace


def get_local_backend_channel() -> str:
    """Get the local backend channel from the test data directory."""
    channel_dir = Path(__file__).parents[2].joinpath(
        "data", "channels", "channels", "pixi_build_backends"
    )
    if channel_dir.is_dir() and any(channel_dir.rglob("repodata.json")):
        return channel_dir.as_uri()
    raise RuntimeError(
        f"Local backend channel not found at {channel_dir}. "
        "Run 'pixi run update-backends-channel' to generate it."
    )


@pytest.fixture(scope="session")
def local_backend_channel_dir() -> Path:
    """Return the path to the local backend channel directory."""
    channel_dir = Path(__file__).parents[2].joinpath(
        "data", "channels", "channels", "pixi_build_backends"
    )
    if not channel_dir.is_dir() or not any(channel_dir.rglob("repodata.json")):
        pytest.skip(
            f"Local backend channel not found at {channel_dir}. "
            "Run 'pixi run update-backends-channel' to generate it."
        )
    return channel_dir


@pytest.fixture(scope="session")
def local_backend_channel_uri(local_backend_channel_dir: Path) -> str:
    """Return the file URI of the local backend channel."""
    return local_backend_channel_dir.as_uri()


@pytest.fixture
def build_data(test_data: Path) -> Path:
    """Return the pixi build test data directory."""
    return test_data.joinpath("pixi-build")


@pytest.fixture
def simple_workspace(
    tmp_pixi_workspace: Path,
    request: pytest.FixtureRequest,
    local_backend_channel_uri: str,
) -> Workspace:
    """Create a simple workspace for build tests."""
    name = request.node.name

    workspace_dir = tmp_pixi_workspace.joinpath("workspace")
    workspace_dir.mkdir()
    shutil.move(tmp_pixi_workspace.joinpath(".pixi"), workspace_dir.joinpath(".pixi"))

    debug_dir = tmp_pixi_workspace.joinpath("debug_dir")
    debug_dir.mkdir()

    recipe = {"package": {"name": name, "version": "1.0.0"}}

    package_rel_dir = "package"

    workspace_manifest = {
        "workspace": {
            "channels": [
                local_backend_channel_uri,
                "https://prefix.dev/conda-forge",
            ],
            "preview": ["pixi-build"],
            "platforms": [CURRENT_PLATFORM],
        },
        "dependencies": {name: {"path": package_rel_dir}},
    }

    package_manifest = {
        "package": {
            "name": name,
            "version": "1.0.0",
            "build": {
                "backend": {
                    "name": "pixi-build-rattler-build",
                    "version": "*",
                    "channels": [
                        local_backend_channel_uri,
                        "https://prefix.dev/conda-forge",
                    ],
                },
            },
        },
    }

    package_dir = workspace_dir.joinpath(package_rel_dir)
    package_dir.mkdir(exist_ok=True)
    recipe_path = package_dir.joinpath("recipe.yaml")

    return Workspace(
        recipe,
        workspace_manifest,
        workspace_dir,
        package_manifest,
        package_dir,
        recipe_path,
        debug_dir,
    )
