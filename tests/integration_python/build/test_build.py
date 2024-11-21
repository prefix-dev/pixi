from pathlib import Path
import shutil
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
            "boltons",
            "--manifest-path",
            manifest_path,
        ],
    )

    parsed_manifest = tomllib.loads(manifest_path.read_text())
    parsed_manifest["tool"]["pixi"]["host-dependencies"] = {"hatchling": "*"}
    parsed_manifest["tool"]["pixi"]["build"] = {
        "build-backend": "pixi-build-python",
        "channels": [
            "https://repo.prefix.dev/pixi-build-backends",
            "https://repo.prefix.dev/conda-forge",
        ],
        "dependencies": ["pixi-build-python"],
    }

    manifest_path.write_text(tomli_w.dumps(parsed_manifest))
    # build it
    verify_cli_command(
        [pixi, "build", "--manifest-path", manifest_path, "--output-dir", manifest_path.parent]
    )

    # really make sure that conda package was built
    package_to_be_built = next(manifest_path.parent.glob("*.conda"))

    assert package_to_be_built.exists()


def test_build_using_rattler_build_backend(pixi: Path, tmp_path: Path) -> None:
    manifest_path = tmp_path / "pixi.toml"

    # Create a new project
    verify_cli_command([pixi, "init", tmp_path])

    parsed_manifest = tomllib.loads(manifest_path.read_text())
    parsed_manifest["host-dependencies"] = {"hatchling": "*"}
    parsed_manifest["build"] = {
        "build-backend": "pixi-build-rattler-build",
        "channels": [
            # until rattler-build backend is merged in pixi build backend,
            # we should use this channel
            "https://repo.prefix.dev/graf",
            "https://repo.prefix.dev/conda-forge",
        ],
        "dependencies": ["pixi-build-rattler-build"],
    }
    manifest_path.write_text(tomli_w.dumps(parsed_manifest))

    # now copy recipe.yaml to the project
    shutil.copy(Path(__file__).parent / "recipes" / "boltons_recipe.yaml", tmp_path / "recipe.yaml")

    # Running pixi build should build the recipe.yaml
    verify_cli_command(
        [pixi, "build", "--manifest-path", manifest_path, "--output-dir", manifest_path.parent],
    )

    # really make sure that conda package was built
    package_to_be_built = next(manifest_path.parent.glob("*.conda"))

    assert "boltons-with-extra" in package_to_be_built.name
    assert package_to_be_built.exists()


def test_build_conda_package_ignoring_recipe(pixi: Path, tmp_path: Path) -> None:
    manifest_path = tmp_path / "pyproject.toml"

    # Create a new project
    verify_cli_command([pixi, "init", tmp_path, "--format", "pyproject"])

    # Add a boltons package to it
    verify_cli_command(
        [
            pixi,
            "add",
            "boltons",
            "--manifest-path",
            manifest_path,
        ],
    )

    parsed_manifest = tomllib.loads(manifest_path.read_text())
    parsed_manifest["tool"]["pixi"]["host-dependencies"] = {"hatchling": "*"}
    parsed_manifest["tool"]["pixi"]["build"] = {
        "build-backend": "pixi-build-python",
        "channels": [
            "https://repo.prefix.dev/pixi-build-backends",
            "https://repo.prefix.dev/conda-forge",
        ],
        "dependencies": ["pixi-build-python"],
    }

    # now copy recipe.yaml to the project
    shutil.copy(Path(__file__).parent / "recipes" / "boltons_recipe.yaml", tmp_path / "recipe.yaml")

    manifest_path.write_text(tomli_w.dumps(parsed_manifest))
    # build it
    verify_cli_command(
        [
            pixi,
            "build",
            "--manifest-path",
            manifest_path,
            "--output-dir",
            manifest_path.parent,
            "--ignore-recipe",
        ]
    )

    # really make sure that conda package was built
    package_to_be_built = next(manifest_path.parent.glob("*.conda"))
    # our recipe has boltons-with-extra name, so we need to be sure that we are building the `pixi.toml`
    # and not the recipe
    assert "test_build_conda_package" in package_to_be_built.name

    assert package_to_be_built.exists()
