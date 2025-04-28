import subprocess

import pytest
from pathlib import Path


PYPROJECT_CONTENT = """
[project]
version = "0.1.0"
name = "test"
requires-python = "== 3.12"
dependencies = [
    "torch @ https://download.pytorch.org/whl/cu124/torch-2.6.0%2Bcu124-cp312-cp312-linux_x86_64.whl#sha256=a393b506844035c0dac2f30ea8478c343b8e95a429f06f3b3cadfc7f53adb597"
]

[build-system]
requires = ["hatchling"]
build-backend = "hatchling.build"

[tool.pixi.pypi-dependencies]
test = { path = ".", editable = true }

[tool.hatch.metadata]
allow-direct-references = true
"""


def test_pypi_url_fragment_in_project_deps(tmp_pixi_workspace: Path, pixi: Path) -> None:
    pyproject_path = tmp_pixi_workspace / "pyproject.toml"
    pyproject_path.write_text(PYPROJECT_CONTENT)

    src_dir = tmp_pixi_workspace / "src" / "test"
    src_dir.mkdir(parents=True, exist_ok=True)
    (src_dir / "__init__.py").touch()

    try:
        subprocess.run(
            [pixi, "install"], cwd=tmp_pixi_workspace, check=True, capture_output=True, text=True
        )
    except subprocess.CalledProcessError as e:
        pytest.fail(f"failed to solve the pypi requirements {e}", pytrace=False)
