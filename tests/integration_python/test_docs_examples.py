import pytest
from pathlib import Path
import shutil
from syrupy.assertion import SnapshotAssertion


from .common import verify_cli_command, repo_root, get_manifest
import sys


pytestmark = pytest.mark.skipif(
    sys.platform.startswith("win"),
    reason="Enable again as soon as pixi build supports windows builds with multiple platforms",
)


@pytest.mark.extra_slow
@pytest.mark.parametrize(
    "pixi_project",
    [
        pytest.param(pixi_project, id=pixi_project.name)
        for pixi_project in repo_root().joinpath("docs/source_files/pixi_projects").iterdir()
    ],
)
def test_doc_pixi_projects(
    pixi_project: Path, pixi: Path, tmp_pixi_workspace: Path, snapshot: SnapshotAssertion
) -> None:
    # TODO: Setting the cache dir shouldn't be necessary!
    env = {"PIXI_CACHE_DIR": str(tmp_pixi_workspace.joinpath("pixi_cache"))}

    # Remove existing .pixi folders
    shutil.rmtree(pixi_project.joinpath(".pixi"), ignore_errors=True)

    # Copy to workspace
    shutil.copytree(pixi_project, tmp_pixi_workspace, dirs_exist_ok=True)

    # Get manifest
    manifest = get_manifest(tmp_pixi_workspace)

    # Run task 'start'
    output = verify_cli_command(
        [pixi, "run", "--locked", "--manifest-path", manifest, "start"], env=env
    )
    assert output.stdout == snapshot
