from pathlib import Path

import pytest

from .common import copytree_with_local_backend, get_manifest, repo_root, verify_cli_command


def _get_minimal_backend_workspaces() -> list[Path]:
    """Get all minimal backend workspace directories for parametrization."""
    base_dir = repo_root().joinpath("tests", "data", "pixi-build", "minimal-backend-workspaces")
    if not base_dir.exists():
        return []
    return [p for p in base_dir.iterdir() if p.is_dir()]


@pytest.mark.slow
@pytest.mark.parametrize(
    "pixi_project",
    [pytest.param(p, id=p.name) for p in _get_minimal_backend_workspaces()],
)
def test_pixi_minimal_backend(pixi_project: Path, pixi: Path, tmp_pixi_workspace: Path) -> None:
    # Copy to workspace
    copytree_with_local_backend(pixi_project, tmp_pixi_workspace, dirs_exist_ok=True)

    # Get manifest
    manifest = get_manifest(tmp_pixi_workspace)

    # Install the environment
    verify_cli_command(
        [pixi, "run", "-v", "--manifest-path", manifest, "start"],
        stdout_contains="Build backend works",
    )


# Enable after the backends have been released
# def test_nameless_versionless(pixi: Path, tmp_pixi_workspace: Path):
#     project_dir = repo_root().joinpath("tests", "data", "pixi_build", "name-and-version-less-package")
#
#     # Copy to workspace
#     shutil.copytree(project_dir, tmp_pixi_workspace, dirs_exist_ok=True)
#
#     # Get manifest
#     manifest = get_manifest(tmp_pixi_workspace)
#
#     # Install the environment
#     verify_cli_command(
#         [pixi, "list", "-v", "--locked", "--manifest-path", manifest],
#         stdout_contains=["rust-app", "1.2.3", "conda"]
#     )
