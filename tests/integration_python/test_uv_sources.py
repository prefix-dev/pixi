from pathlib import Path
import pytest
import shutil
import sys

from .common import verify_cli_command


@pytest.mark.slow
def test_install_with_uv_sources(
    pixi: Path,
    tmp_pixi_workspace: Path,
    test_data: Path,
) -> None:
    shutil.copytree(test_data / "uv-sources-non-root", tmp_pixi_workspace, dirs_exist_ok=True)
    verify_cli_command(
        [pixi, "install", "--manifest-path", tmp_pixi_workspace],
    )
    # Check if dist-info is available for local-library2
    #
    if sys.platform.startswith("win"):
        python_dir = ""
    else:
        python_dir = "python3.13"

    assert (
        tmp_pixi_workspace
        / ".pixi"
        / "envs"
        / "default"
        / "lib"
        / python_dir
        / "site-packages"
        / "local_library2-0.1.0.dist-info"
    ).exists()
