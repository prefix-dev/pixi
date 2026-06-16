"""Pytest fixtures for pixi-build integration tests."""

import shutil
from pathlib import Path
from typing import Any

import pytest

from .common import CURRENT_PLATFORM, Workspace


@pytest.fixture
def build_data(test_data: Path) -> Path:
    """Return the pixi build test data directory."""
    return test_data.joinpath("pixi-build")


@pytest.fixture
def simple_workspace(
    tmp_pixi_workspace: Path,
    request: pytest.FixtureRequest,
) -> Workspace:
    """Create a simple workspace for build tests."""
    name: str = request.node.name  # pyright: ignore[reportUnknownMemberType,reportUnknownVariableType]

    # Make sure the tmp workspace is cleared before each test
    # This is important for windows where we might have issues with file locks if we try to delete after the test
    for item in tmp_pixi_workspace.iterdir():
        if item.is_dir():
            shutil.rmtree(item)
        else:
            item.unlink()

    workspace_dir = tmp_pixi_workspace.joinpath("workspace")
    workspace_dir.mkdir()

    debug_dir = tmp_pixi_workspace.joinpath("debug_dir")
    debug_dir.mkdir()

    recipe: dict[str, Any] = {"package": {"name": name, "version": "1.0.0"}}

    package_rel_dir = "package"

    workspace_manifest: dict[str, Any] = {
        "workspace": {
            "channels": ["https://prefix.dev/conda-forge"],
            "preview": ["pixi-build"],
            "platforms": [CURRENT_PLATFORM],
        },
        "dependencies": {name: {"path": package_rel_dir}},
    }

    package_manifest: dict[str, Any] = {
        "package": {
            "name": name,
            "version": "1.0.0",
            "build": {
                "backend": {
                    "name": "pixi-build-rattler-build",
                    "version": "*",
                },
            },
        },
    }

    package_dir = workspace_dir.joinpath(package_rel_dir)
    package_dir.mkdir()
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
