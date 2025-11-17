import shutil
import sys
from pathlib import Path

import pytest

from .common import repo_root, verify_cli_command

pytestmark = pytest.mark.skipif(
    sys.platform.startswith("win"),
    reason="Enable again as soon as pixi build supports windows builds with multiple platforms",
)


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

    # Build the packages for all variants
    verify_cli_command(
        [pixi, "build", "--path", tmp_pixi_workspace, "--output-dir", str(tmp_pixi_workspace)],
    )

    # Check that work directories exist and are separate for each variant
    work_dir = tmp_pixi_workspace / ".pixi" / "build" / "work"
    assert work_dir.exists(), "Work directory should exist"

    # Get all work directories (should be different for py311 and py312)
    work_dirs = list(work_dir.glob("python_rich-*/*"))

    # Should have at least 2 work directories (one per Python variant)
    assert len(work_dirs) >= 2, (
        f"Expected at least 2 work directories for different Python variants, "
        f"found {len(work_dirs)}: {[d.name for d in work_dirs]}"
    )
