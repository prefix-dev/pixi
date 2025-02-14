from pathlib import Path
import pytest
from ..common import CURRENT_PLATFORM


@pytest.fixture
def build_data(test_data: Path) -> Path:
    """
    Returns the pixi build test data
    """
    return test_data.joinpath("pixi_build")


@pytest.fixture
def examples_dir() -> Path:
    """
    Returns the path to the examples directory in the root of the repository
    """
    return Path(__file__).parents[3].joinpath("examples").resolve()


@pytest.fixture
def simple_workspace(tmp_pixi_workspace: Path, request: pytest.FixtureRequest) -> Path:
    name = request.node.name

    recipe = f"""
    package:
      name: {name}
      version: 1.0.0
    """
    recipe_path = tmp_pixi_workspace.joinpath("recipe.yaml")
    recipe_path.write_text(recipe)

    manifest = f"""
    [workspace]
    channels = ["https://prefix.dev/pixi-build-backends", "https://prefix.dev/conda-forge"]
    preview = ["pixi-build"]
    platforms = ["{CURRENT_PLATFORM}"]
    name = "{name}"
    version = "1.0.0"

    [dependencies]
    {name} = {{ path = "." }}

    [package.build.backend]
    name = "pixi-build-rattler-build"
    version = "0.1.*"

    [package.build.configuration]
    debug-dir = "{tmp_pixi_workspace}"
    """
    manifest_path = tmp_pixi_workspace.joinpath("pixi.toml")
    manifest_path.write_text(manifest)
    return manifest_path
