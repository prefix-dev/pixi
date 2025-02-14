from pathlib import Path
import shutil
import json
import pytest

from ..common import ExitCode, verify_cli_command
import tomllib
import tomli_w


@pytest.mark.slow
def test_build_conda_package(
    pixi: Path,
    simple_workspace: Path,
) -> None:
    verify_cli_command(
        [
            pixi,
            "build",
            "--manifest-path",
            simple_workspace,
            "--output-dir",
            simple_workspace.parent,
        ],
    )

    # really make sure that conda package was built
    built_packages = list(simple_workspace.parent.glob("*.conda"))

    assert len(built_packages) == 1
    assert built_packages[0].exists()


@pytest.mark.extra_slow
def test_build_using_rattler_build_backend(
    pixi: Path,
    tmp_pixi_workspace: Path,
    build_data: Path,
) -> None:
    test_data = build_data.joinpath("rattler-build-backend")
    shutil.copytree(test_data / "pixi", tmp_pixi_workspace / "pixi")
    shutil.copyfile(
        test_data / "recipes/smokey/recipe.yaml", tmp_pixi_workspace / "pixi/recipe.yaml"
    )

    manifest_path = tmp_pixi_workspace / "pixi" / "pixi.toml"

    # Running pixi build should build the recipe.yaml
    verify_cli_command(
        [pixi, "build", "--manifest-path", manifest_path, "--output-dir", manifest_path.parent],
    )

    # really make sure that conda package was built
    package_to_be_built = next(manifest_path.parent.glob("*.conda"))

    assert "smokey" in package_to_be_built.name
    assert package_to_be_built.exists()


@pytest.mark.extra_slow
def test_smokey(pixi: Path, build_data: Path, tmp_pixi_workspace: Path) -> None:
    test_data = build_data.joinpath("rattler-build-backend")
    # copy the whole smokey project to the tmp_pixi_workspace
    shutil.copytree(test_data, tmp_pixi_workspace / "test_data")
    manifest_path = tmp_pixi_workspace / "test_data" / "smokey" / "pixi.toml"
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


def test_source_change_trigger_rebuild(pixi: Path, simple_workspace: Path) -> None:
    env = {
        "PIXI_BUILD_BACKEND_OVERRIDE": "pixi-build-rattler-build=/var/home/julian/Projekte/github.com/prefix-dev/pixi-build-backends/target/release/pixi-build-rattler-build",
    }
    verify_cli_command(
        [
            pixi,
            "install",
            "--manifest-path",
            simple_workspace,
        ],
        env=env,
    )

    conda_build_params = simple_workspace.parent.joinpath("conda_build_params.json")

    assert conda_build_params.is_file()

    # Remove the conda build params to get a clean state
    conda_build_params.unlink()

    # Touch the recipe
    simple_workspace.parent.joinpath("recipe.yaml").touch()

    verify_cli_command(
        [
            pixi,
            "install",
            "--manifest-path",
            simple_workspace,
        ],
        env=env,
    )

    # Touching the recipe should trigger a rebuild and therefore create the file
    assert conda_build_params.is_file()


def test_host_dependency_change_trigger_rebuild(
    pixi: Path, simple_workspace: Path, dummy_channel_1: Path
) -> None:
    env = {
        "PIXI_BUILD_BACKEND_OVERRIDE": "pixi-build-rattler-build=/var/home/julian/Projekte/github.com/prefix-dev/pixi-build-backends/target/release/pixi-build-rattler-build",
    }
    verify_cli_command(
        [
            pixi,
            "install",
            "--manifest-path",
            simple_workspace,
        ],
        env=env,
    )

    conda_build_params = simple_workspace.parent.joinpath("conda_build_params.json")

    assert conda_build_params.is_file()

    # Remove the conda build params to get a clean state
    conda_build_params.unlink()

    # Add dummy-b to host-dependencies
    manifest_data = tomllib.loads(simple_workspace.read_text())
    manifest_data["package"].setdefault("host-dependencies", {})["dummy-b"] = {
        "version": "*",
        "channel": dummy_channel_1,
    }
    simple_workspace.write_text(tomli_w.dumps(manifest_data))
    verify_cli_command(
        [
            pixi,
            "install",
            "--manifest-path",
            simple_workspace,
        ],
        env=env,
    )

    # modifying the host-dependencies should trigger a rebuild and therefore create a file
    assert conda_build_params.is_file()


@pytest.mark.slow
def test_editable_pyproject(pixi: Path, build_data: Path, tmp_pixi_workspace: Path) -> None:
    """
    This one tries to run the Python based rich example project,
    installed as a normal package by overriding with an environment variable.
    """
    project = "editable-pyproject"
    test_data = build_data.joinpath(project)

    target_dir = tmp_pixi_workspace.joinpath(project)
    shutil.copytree(test_data, target_dir)
    manifest_path = target_dir.joinpath("pyproject.toml")

    verify_cli_command(
        [
            pixi,
            "install",
            "--manifest-path",
            manifest_path,
        ],
    )

    # Verify that package is installed as editable
    verify_cli_command(
        [
            pixi,
            "run",
            "--manifest-path",
            manifest_path,
            "check-editable",
        ],
        stdout_contains="The package is installed as editable.",
    )


@pytest.mark.slow
def test_non_editable_pyproject(pixi: Path, build_data: Path, tmp_pixi_workspace: Path) -> None:
    """
    This one tries to run the Python based rich example project,
    installed as a normal package by overriding with an environment variable.
    """
    project = "editable-pyproject"
    test_data = build_data.joinpath(project)

    target_dir = tmp_pixi_workspace.joinpath(project)
    shutil.copytree(test_data, target_dir)
    manifest_path = target_dir.joinpath("pyproject.toml")

    # TODO: Setting the cache dir shouldn't be necessary!
    env = {
        "BUILD_EDITABLE_PYTHON": "false",
        "PIXI_CACHE_DIR": str(tmp_pixi_workspace.joinpath("pixi_cache")),
    }

    verify_cli_command(
        [
            pixi,
            "install",
            "--manifest-path",
            manifest_path,
        ],
        env=env,
    )

    # Verify that package is installed as editable
    verify_cli_command(
        [
            pixi,
            "run",
            "--manifest-path",
            manifest_path,
            "check-editable",
        ],
        ExitCode.FAILURE,
        env=env,
        stdout_contains="The package is not installed as editable.",
    )
