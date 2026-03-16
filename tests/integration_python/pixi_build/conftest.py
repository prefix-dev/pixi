"""Pytest fixtures for pixi-build integration tests."""

import os
import shutil
from pathlib import Path
from typing import Any

import pytest

from .common import CURRENT_PLATFORM, Workspace, exec_extension, repo_root


@pytest.fixture(scope="session", autouse=True)
def setup_build_backend_override(request: pytest.FixtureRequest) -> None:
    """
    Sets up PIXI_BUILD_BACKEND_OVERRIDE for Rust backends.

    Points to binaries in target/pixi/{build_type}/ based on --pixi-build option.
    """
    build_type = request.config.getoption("--pixi-build")
    backends_bin_dir = repo_root() / "target" / "pixi" / build_type

    if not backends_bin_dir.is_dir():
        return  # Skip if not built yet

    backends = [
        "pixi-build-cmake",
        "pixi-build-python",
        "pixi-build-rattler-build",
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
