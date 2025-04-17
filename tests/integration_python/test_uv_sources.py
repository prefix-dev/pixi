from pathlib import Path
import pytest
import shutil

from .common import ExitCode, verify_cli_command

@pytest.mark.slow
def test_install_with_uv_sources(
    pixi: Path, tmp_pixi_workspace: Path, test_data: Path,
):
    shutil.copytree(test_data / "uv-sources-non-root", tmp_pixi_workspace, dirs_exist_ok=True)
    verify_cli_command(
        [pixi, "install", "--manifest-path", tmp_pixi_workspace],
        expected_exit_code=ExitCode.SUCCESS,
    )
