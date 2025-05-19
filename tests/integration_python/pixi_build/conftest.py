from dataclasses import dataclass
from pathlib import Path
from typing import Any

import yaml
import tomli_w
import pytest
import shutil
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


@dataclass
class Workspace:
    recipe: dict[str, Any]
    manifest: dict[str, Any]
    workspace_dir: Path
    debug_dir: Path

    def write_files(self) -> None:
        recipe_path = self.workspace_dir.joinpath("recipe.yaml")
        recipe_path.write_text(yaml.dump(self.recipe))
        manifest_path = self.workspace_dir.joinpath("pixi.toml")
        manifest_path.write_text(tomli_w.dumps(self.manifest))


@pytest.fixture
def simple_workspace(tmp_pixi_workspace: Path, request: pytest.FixtureRequest) -> Workspace:
    name = request.node.name

    workspace_dir = tmp_pixi_workspace.joinpath("workspace")
    workspace_dir.mkdir()
    shutil.move(tmp_pixi_workspace.joinpath(".pixi"), workspace_dir.joinpath(".pixi"))

    debug_dir = tmp_pixi_workspace.joinpath("debug_dir")
    debug_dir.mkdir()

    recipe = {"package": {"name": name, "version": "1.0.0"}}

    manifest = {
        "workspace": {
            "channels": [
                "https://prefix.dev/pixi-build-backends",
                "https://prefix.dev/conda-forge",
            ],
            "preview": ["pixi-build"],
            "platforms": [CURRENT_PLATFORM],
            "name": name,
            "version": "1.0.0",
        },
        "dependencies": {name: {"path": "."}},
        "package": {
            "build": {
                "backend": {"name": "pixi-build-rattler-build", "version": "0.1.*"},
                "configuration": {"debug-dir": str(debug_dir)},
            }
        },
    }

    return Workspace(recipe, manifest, workspace_dir, debug_dir)
