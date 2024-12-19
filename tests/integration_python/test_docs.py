import pytest
from pathlib import Path
import shutil
import sys

from .common import verify_cli_command, ExitCode, repo_root, current_platform


@pytest.mark.skipif(
    sys.platform.startswith("win"),
    reason="Enable again as soon as pixi build supports windows builds with multiple platforms",
)
@pytest.mark.extra_slow
def test_doc_pixi_projects(pixi: Path, tmp_pixi_workspace: Path, doc_pixi_projects: Path) -> None:
    # TODO: Setting the cache dir shouldn't be necessary!
    env = {"PIXI_CACHE_DIR": str(tmp_pixi_workspace.joinpath("pixi_cache"))}
    target_dir = tmp_pixi_workspace.joinpath("pixi_projects")
    shutil.copytree(doc_pixi_projects, target_dir)

    for pixi_project in target_dir.iterdir():
        shutil.rmtree(pixi_project.joinpath(".pixi"), ignore_errors=True)
        manifest = pixi_project.joinpath("pixi.toml")
        # Run the test command
        verify_cli_command(
            [pixi, "run", "--manifest-path", manifest, "start"], ExitCode.SUCCESS, env=env
        )


@pytest.mark.extra_slow
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

    verify_cli_command(
        [pixi, "info"],
        ExitCode.SUCCESS,
    )

    # Only solve if the platform is supported
    if (
        current_platform()
        in verify_cli_command(
            [pixi, "project", "platform", "ls", "--manifest-path", manifest],
            ExitCode.SUCCESS,
        ).stdout
    ):
        # Run the installation
        verify_cli_command(
            [pixi, "install", "--manifest-path", manifest],
            ExitCode.SUCCESS,
        )

        # Cleanup the workspace
        shutil.rmtree(tmp_pixi_workspace)
