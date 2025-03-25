from .common import verify_cli_command
import pytest
from pathlib import Path
import shutil
import tomllib
import tomli_w


@pytest.fixture
def reinstall_workspace(tmp_pixi_workspace: Path, mock_projects: Path) -> Path:
    test_rebuild_src = mock_projects / "test-rebuild"
    shutil.rmtree(test_rebuild_src.joinpath(".pixi"), ignore_errors=True)
    shutil.copytree(test_rebuild_src, tmp_pixi_workspace, dirs_exist_ok=True)

    # Enable debug logging
    packages = ["pixi_build_package", "pixi_build_package_dev"]
    for package in packages:
        package_dir = tmp_pixi_workspace / package
        package_manifest = package_dir / "pixi.toml"
        manifest_dict = tomllib.loads(package_manifest.read_text())
        manifest_dict["package"]["build"]["configuration"] = {"debug-dir": str(package_dir)}
        package_manifest.write_text(tomli_w.dumps(manifest_dict))

    return tmp_pixi_workspace


@pytest.mark.slow
def test_pixi_reinstall_default_env(pixi: Path, reinstall_workspace: Path) -> None:
    env = {
        "PIXI_CACHE_DIR": str(reinstall_workspace.joinpath("pixi_cache")),
    }
    manifest = reinstall_workspace.joinpath("pixi.toml")
    conda_metadata_params = reinstall_workspace.joinpath(
        "pixi_build_package", "conda_metadata_params.json"
    )
    conda_build_params = reinstall_workspace.joinpath(
        "pixi_build_package", "conda_build_params.json"
    )

    # Check that packages return "number 1"
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "pypi-package-main"],
        stdout_contains="PyPI is number 1",
        env=env,
    )
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "pixi-build-package-main"],
        stdout_contains="Pixi Build is number 1",
        env=env,
    )

    # In order to build pixi-build-package-main, getMetadata and build has been called
    assert conda_metadata_params.is_file()
    assert conda_build_params.is_file()

    # Delete the files to get a clean state
    conda_metadata_params.unlink()
    conda_build_params.unlink()

    # Modify the Python files
    for package in ["pypi_package", "pixi_build_package"]:
        init_py = reinstall_workspace.joinpath(package, "src", package, "__init__.py")
        init_py.write_text(init_py.read_text().replace("1", "2"))

    # That shouldn't trigger a re-install, so running still returns "number 1"
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "pypi-package-main"],
        stdout_contains="PyPI is number 1",
        env=env,
    )
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "pixi-build-package-main"],
        stdout_contains="Pixi Build is number 1",
        env=env,
    )

    # Everything pixi-build related is cached, no remote procedure was called
    assert not conda_metadata_params.is_file()
    assert not conda_build_params.is_file()

    # After re-installing pypi-package, it should return "number 2"
    # pixi-build-package, should not be rebuild and therefore still return "number 1"
    verify_cli_command([pixi, "reinstall", "--manifest-path", manifest, "pypi_package"], env=env)
    assert not conda_metadata_params.is_file()
    assert not conda_build_params.is_file()
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "pypi-package-main"],
        stdout_contains="PyPI is number 2",
        env=env,
    )
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "pixi-build-package-main"],
        stdout_contains="Pixi Build is number 1",
        env=env,
    )

    # After re-installing the whole default environment,
    # both should return "number 2"
    verify_cli_command([pixi, "reinstall", "--manifest-path", manifest], env=env)
    assert not conda_metadata_params.is_file()
    assert conda_build_params.is_file()
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "pypi-package-main"],
        stdout_contains="PyPI is number 2",
        env=env,
    )
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "pixi-build-package-main"],
        stdout_contains="Pixi Build is number 2",
        env=env,
    )


@pytest.mark.slow
def test_pixi_reinstall_multi_env(pixi: Path, reinstall_workspace: Path) -> None:
    env = {
        "PIXI_CACHE_DIR": str(reinstall_workspace.joinpath("pixi_cache")),
    }
    manifest = reinstall_workspace.joinpath("pixi.toml")
    conda_metadata_params = reinstall_workspace.joinpath(
        "pixi_build_package", "conda_metadata_params.json"
    )
    conda_build_params = reinstall_workspace.joinpath(
        "pixi_build_package", "conda_build_params.json"
    )
    conda_metadata_params_dev = reinstall_workspace.joinpath(
        "pixi_build_package_dev", "conda_metadata_params.json"
    )
    conda_build_params_dev = reinstall_workspace.joinpath(
        "pixi_build_package_dev", "conda_build_params.json"
    )

    # Check that packages return "number 1" in default environment
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "pypi-package-main"],
        stdout_contains="PyPI is number 1",
        env=env,
    )
    verify_cli_command(
        [
            pixi,
            "run",
            "--manifest-path",
            manifest,
            "pixi-build-package-main",
        ],
        stdout_contains="Pixi Build is number 1",
        env=env,
    )

    # Check that packages return "number 1" in dev environment
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "--environment", "dev", "pypi-package-dev-main"],
        stdout_contains="PyPI dev is number 1",
        env=env,
    )
    verify_cli_command(
        [
            pixi,
            "run",
            "--manifest-path",
            manifest,
            "--environment",
            "dev",
            "pixi-build-package-dev-main",
        ],
        stdout_contains="Pixi Build dev is number 1",
        env=env,
    )

    # In order to build pixi-build-package-main, getMetadata and build has been called
    assert conda_metadata_params_dev.is_file()
    assert conda_build_params_dev.is_file()

    # Delete the files to get a clean state
    conda_metadata_params_dev.unlink()
    conda_build_params_dev.unlink()

    # Modify the Python files
    for package in [
        "pypi_package",
        "pixi_build_package",
        "pypi_package_dev",
        "pixi_build_package_dev",
    ]:
        init_py = reinstall_workspace.joinpath(package, "src", package, "__init__.py")
        init_py.write_text(init_py.read_text().replace("1", "2"))

    # That shouldn't trigger a re-install, so running still returns "number 1"
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "--environment", "dev", "pypi-package-dev-main"],
        stdout_contains="PyPI dev is number 1",
        env=env,
    )
    verify_cli_command(
        [
            pixi,
            "run",
            "--manifest-path",
            manifest,
            "--environment",
            "dev",
            "pixi-build-package-dev-main",
        ],
        stdout_contains="Pixi Build dev is number 1",
        env=env,
    )

    # Everything pixi-build related is cached, no remote procedure was called
    assert not conda_metadata_params_dev.is_file()
    assert not conda_build_params_dev.is_file()

    # After re-building pixi_build_package_dev, it should return "number 2"
    # pypi_package_dev, should still return "number 1"
    verify_cli_command(
        [
            pixi,
            "reinstall",
            "--manifest-path",
            manifest,
            "--environment",
            "dev",
            "pixi_build_package_dev",
        ],
        env=env,
    )
    assert not conda_metadata_params_dev.is_file()
    assert conda_build_params_dev.is_file()
    verify_cli_command(
        [
            pixi,
            "run",
            "--manifest-path",
            manifest,
            "--environment",
            "dev",
            "pixi-build-package-dev-main",
        ],
        stdout_contains="Pixi Build dev is number 2",
        env=env,
    )
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "--environment", "dev", "pypi-package-dev-main"],
        stdout_contains="PyPI dev is number 1",
        env=env,
    )

    # After re-installing both packages in the "dev" environment,
    # both should return "number 2"
    verify_cli_command(
        [
            pixi,
            "reinstall",
            "--manifest-path",
            manifest,
            "--environment",
            "dev",
            "pypi_package_dev",
            "pixi_build_package_dev",
        ],
        env=env,
    )
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "--environment", "dev", "pypi-package-dev-main"],
        stdout_contains="PyPI dev is number 2",
        env=env,
    )
    verify_cli_command(
        [
            pixi,
            "run",
            "--manifest-path",
            manifest,
            "--environment",
            "dev",
            "pixi-build-package-dev-main",
        ],
        stdout_contains="Pixi Build dev is number 2",
        env=env,
    )

    # In the default environment, it should still be "number 1"
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "pypi-package-main"],
        stdout_contains="PyPI is number 1",
        env=env,
    )
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "pixi-build-package-main"],
        stdout_contains="Pixi Build is number 1",
        env=env,
    )

    # Delete the files to get a clean state
    conda_metadata_params.unlink()
    conda_build_params.unlink()

    # After reinstalling all environments,
    # also the default environment should be "number 2"
    verify_cli_command([pixi, "reinstall", "--manifest-path", manifest, "--all"], env=env)
    assert not conda_metadata_params.is_file()
    assert conda_build_params.is_file()
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "pypi-package-main"],
        stdout_contains="PyPI is number 2",
        env=env,
    )
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "pixi-build-package-main"],
        stdout_contains="Pixi Build is number 2",
        env=env,
    )
