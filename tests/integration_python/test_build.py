import shutil
from pathlib import Path

import pytest

from .common import repo_root, verify_cli_command


@pytest.mark.extra_slow
def test_workspace_variants_separate_work_directories(
    pixi: Path,
    tmp_pixi_workspace: Path,
) -> None:
    """Test that building with multiple Python variants creates separate work directories.

    This test verifies the fix for issue #4878 where .pyc files from different
    Python versions would accumulate in the same work directory, causing package
    sizes to grow progressively.

    The fix ensures that each variant combination gets its own work directory by
    including variants in the work directory key hash.
    """
    # Find the workspace_variants project
    workspace_variants_project = repo_root().joinpath(
        "docs/source_files/pixi_workspaces/pixi_build/workspace_variants"
    )

    # Remove existing .pixi folders
    shutil.rmtree(workspace_variants_project.joinpath(".pixi"), ignore_errors=True)

    # Copy to workspace
    shutil.copytree(workspace_variants_project, tmp_pixi_workspace, dirs_exist_ok=True)

    # TODO: Needs to publish to a local directory instead to be fully compatible with pixi build
    verify_cli_command(
        [pixi, "publish", "--path", tmp_pixi_workspace, f"file://{tmp_pixi_workspace}"],
    )

    # Check that the package's bld root exists.
    # Layout: .pixi/bld/<pkg>/<workspace_key>/ (one workspace_key per variant).
    package_bld_dir = tmp_pixi_workspace / ".pixi" / "bld" / "python_rich"
    assert package_bld_dir.exists(), "Package build directory should exist"

    # Should have at least 2 workspace directories (one per Python variant).
    workspace_dirs = [d for d in package_bld_dir.iterdir() if d.is_dir()]
    assert len(workspace_dirs) >= 2, (
        f"Expected at least 2 workspace directories for different Python variants, "
        f"found {len(workspace_dirs)}: {[d.name for d in workspace_dirs]}"
    )
