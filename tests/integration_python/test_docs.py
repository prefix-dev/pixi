import pytest
from pathlib import Path
import shutil

from .common import ExitCode, verify_cli_command


@pytest.mark.slow
def test_doc_pixi_projects(pixi: Path, tmp_pixi_workspace: Path, doc_pixi_projects: Path) -> None:
    # TODO: Setting the cache dir shouldn't be necessary!
    env = {"PIXI_CACHE_DIR": str(tmp_pixi_workspace.joinpath("pixi_cache"))}
    target_dir = tmp_pixi_workspace.joinpath("pixi_projects")
    shutil.copytree(doc_pixi_projects, target_dir)

    for pixi_project in target_dir.iterdir():
        shutil.rmtree(pixi_project.joinpath(".pixi"))
        manifest = pixi_project.joinpath("pixi.toml")
        # Run the test command
        verify_cli_command(
            [pixi, "run", "--manifest-path", manifest, "start"], ExitCode.SUCCESS, env=env
        )
