from pathlib import Path
import tomllib

import tomli_w

from ..common import verify_cli_command


def test_build_conda_package(pixi: Path, tmp_path: Path) -> None:
    manifest_path = tmp_path / "pyproject.toml"

    # Create a new project
    verify_cli_command([pixi, "init", tmp_path, "--format", "pyproject"])

    # Add a boltons package to it
    verify_cli_command(
        [
            pixi,
            "add",
            "boltons" "--manifest-path",
            manifest_path,
        ],
    )

    parsed_manifest = tomllib.loads(manifest_path.read_text())
    parsed_manifest["tool"]["pixi"]["host-dependencies"] = {"hatchling": "*"}
    parsed_manifest["tool"]["pixi"]["build"] = {
        "build-backend": "pixi-build-python",
        "channels": ["https://prefix.dev/graf", "https://fast.prefix.dev/conda-forge"],
        "dependencies": ["pixi-build-python", "hatchling"],
    }

    manifest_path.write_text(tomli_w.dumps(parsed_manifest))
    # build it
    verify_cli_command(
        [pixi, "build", "--manifest-path", manifest_path, "--output-dir", manifest_path.parent]
    )

    # really make sure that conda package was built
    # finda a conda package
    package_to_be_built = next(manifest_path.parent.glob("*.conda"))

    assert package_to_be_built.exists()
