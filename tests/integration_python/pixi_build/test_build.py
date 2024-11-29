from pathlib import Path
import shutil
import json


from ..common import verify_cli_command


def get_data_dir(backend: str | None = None) -> Path:
    """
    Returns the path to the test-data directory next to the tests
    """
    if backend is None:
        return Path(__file__).parent / "test-data"
    else:
        return Path(__file__).parent / "test-data" / backend


def examples_dir() -> Path:
    """
    Returns the path to the examples directory in the root of the repository
    """
    return (Path(__file__).parent / "../../../examples").resolve()


def test_build_conda_package(pixi: Path, tmp_path: Path) -> None:
    """
    This one tries to build the example flask hello world project
    """
    pyproject = examples_dir() / "flask-hello-world-pyproject"
    shutil.copytree(pyproject, tmp_path / "pyproject")

    manifest_path = tmp_path / "pyproject" / "pyproject.toml"
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

    # build it
    verify_cli_command(
        [pixi, "build", "--manifest-path", manifest_path, "--output-dir", manifest_path.parent]
    )

    # really make sure that conda package was built
    package_to_be_built = next(manifest_path.parent.glob("*.conda"))

    assert package_to_be_built.exists()


def test_build_using_rattler_build_backend(pixi: Path, tmp_path: Path) -> None:
    test_data = get_data_dir("rattler-build-backend")
    shutil.copytree(test_data / "pixi", tmp_path / "pixi")
    shutil.copyfile(test_data / "recipes/smokey/recipe.yaml", tmp_path / "pixi/recipe.yaml")

    manifest_path = tmp_path / "pixi" / "pixi.toml"

    # Running pixi build should build the recipe.yaml
    verify_cli_command(
        [pixi, "build", "--manifest-path", manifest_path, "--output-dir", manifest_path.parent],
    )

    # really make sure that conda package was built
    package_to_be_built = next(manifest_path.parent.glob("*.conda"))

    assert "smokey" in package_to_be_built.name
    assert package_to_be_built.exists()


def test_smokey(pixi: Path, tmp_path: Path) -> None:
    test_data = get_data_dir("rattler-build-backend")
    # copy the whole smokey project to the tmp_path
    shutil.copytree(test_data, tmp_path / "test_data")
    manifest_path = tmp_path / "test_data" / "smokey" / "pixi.toml"
    verify_cli_command(
        [
            pixi,
            "install",
            "--manifest-path",
            manifest_path,
        ]
    )

    # load the json file
    conda_meta = (
        (manifest_path.parent / ".pixi/envs/default/conda-meta").glob("smokey-*.json").__next__()
    )
    metadata = json.loads(conda_meta.read_text())

    assert metadata["name"] == "smokey"
