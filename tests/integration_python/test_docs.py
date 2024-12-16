import pytest
from pathlib import Path
import shutil

from .common import ExitCode, verify_cli_command


@pytest.mark.slow
def test_doc_pixi_projects(pixi: Path, tmp_pixi_workspace: Path) -> None:
    # TODO: Setting the cache dir shouldn't be necessary!
    env = {"PIXI_CACHE_DIR": str(tmp_pixi_workspace.joinpath("pixi_cache"))}
    pixi_project_dir = Path(__file__).parents[2].joinpath("docs", "source_files", "pixi_projects")
    target_dir = tmp_pixi_workspace.joinpath("pixi_projects")
    shutil.copytree(pixi_project_dir, target_dir)

    for pixi_project in target_dir.iterdir():
        shutil.rmtree(pixi_project.joinpath(".pixi"))
        manifest = pixi_project.joinpath("pixi.toml")
        # Run the test command
        verify_cli_command(
            [pixi, "run", "--manifest-path", manifest, "test"], ExitCode.SUCCESS, env=env
        )
