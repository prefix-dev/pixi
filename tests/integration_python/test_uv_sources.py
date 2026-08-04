import json
from pathlib import Path
import pytest
import shutil
import sys

from .common import verify_cli_command


def site_packages(workspace: Path) -> Path:
    if sys.platform.startswith("win"):
        python_dir = ""
    else:
        python_dir = "python3.13"
    return workspace / ".pixi" / "envs" / "default" / "lib" / python_dir / "site-packages"


def is_editable(workspace: Path, package: str, version: str = "0.1.0") -> bool:
    direct_url = site_packages(workspace) / f"{package}-{version}.dist-info" / "direct_url.json"
    return json.loads(direct_url.read_text())["dir_info"].get("editable", False)


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
    assert (site_packages(tmp_pixi_workspace) / "local_library2-0.1.0.dist-info").exists()

    # `local-library` declares `local-library2` editable in its `[tool.uv.sources]`,
    # and the workspace manifest doesn't mention it, so it installs editable.
    assert is_editable(tmp_pixi_workspace, "local_library")
    assert is_editable(tmp_pixi_workspace, "local_library2")


@pytest.mark.slow
def test_manifest_overrides_uv_sources_editable(
    pixi: Path,
    tmp_pixi_workspace: Path,
    test_data: Path,
) -> None:
    """A path dependency in the manifest decides editability, even when a
    `[tool.uv.sources]` entry of a dependency disagrees."""
    shutil.copytree(test_data / "uv-sources-non-root", tmp_pixi_workspace, dirs_exist_ok=True)
    manifest = tmp_pixi_workspace / "pyproject.toml"
    manifest.write_text(manifest.read_text() + 'local-library2 = { path = "./local-library2" }\n')

    verify_cli_command(
        [pixi, "install", "--manifest-path", tmp_pixi_workspace],
    )

    assert is_editable(tmp_pixi_workspace, "local_library")
    assert not is_editable(tmp_pixi_workspace, "local_library2")
