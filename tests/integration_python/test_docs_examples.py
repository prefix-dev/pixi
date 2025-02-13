import pytest
from pathlib import Path
import shutil
from syrupy.assertion import SnapshotAssertion


from .common import verify_cli_command, repo_root, current_platform, get_manifest
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
    # Remove existing .pixi folders
    shutil.rmtree(pixi_project.joinpath(".pixi"), ignore_errors=True)

    # Copy to workspace
    shutil.copytree(pixi_project, tmp_pixi_workspace, dirs_exist_ok=True)

    # Get manifest
    manifest = get_manifest(tmp_pixi_workspace)

    # Run task 'start'
    output = verify_cli_command(
        [pixi, "run", "--locked", "--manifest-path", manifest, "start"],
    )
    assert output.stdout == snapshot


@pytest.mark.extra_slow
@pytest.mark.timeout(200)
@pytest.mark.parametrize(
    "manifest",
    [
        pytest.param(manifest, id=manifest.stem)
        for manifest in repo_root().joinpath("docs/source_files/").glob("**/pytorch-*.toml")
    ],
)
def test_pytorch_documentation_examples(
    manifest: Path,
    pixi: Path,
    tmp_pixi_workspace: Path,
) -> None:
    # Copy the manifest to the tmp workspace
    toml = manifest.read_text()
    toml_name = "pyproject.toml" if "pyproject_tomls" in str(manifest) else "pixi.toml"
    manifest = tmp_pixi_workspace.joinpath(toml_name)
    manifest.write_text(toml)

    # Only solve if the platform is supported
    if (
        current_platform()
        in verify_cli_command(
            [pixi, "project", "platform", "ls", "--manifest-path", manifest],
        ).stdout
    ):
        # Run the installation
        verify_cli_command(
            [pixi, "install", "--manifest-path", manifest],
        )
