from .common import verify_cli_command
import pytest
from pathlib import Path
import shutil


@pytest.fixture
def reinstall_workspace(tmp_pixi_workspace: Path, mock_projects: Path) -> Path:
    test_rebuild_src = mock_projects / "test-rebuild"
    shutil.rmtree(test_rebuild_src.joinpath(".pixi"), ignore_errors=True)
    shutil.copytree(test_rebuild_src, tmp_pixi_workspace, dirs_exist_ok=True)

    return tmp_pixi_workspace


@pytest.mark.extra_slow
def test_pixi_reinstall_default_env(pixi: Path, reinstall_workspace: Path) -> None:
    env = {
        "PIXI_CACHE_DIR": str(reinstall_workspace.joinpath("pixi_cache")),
    }
    manifest = reinstall_workspace.joinpath("pixi.toml")

    # Check that packages return "number 1"
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "pypi-package-main"],
        stdout_contains="PyPI is number 1",
        env=env,
    )

    # Modify the Python files
    init_py = reinstall_workspace.joinpath("pypi_package", "src", "pypi_package", "__init__.py")
    init_py.write_text(init_py.read_text().replace("1", "2"))

    # That shouldn't trigger a re-install, so running still returns "number 1"
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "pypi-package-main"],
        stdout_contains="PyPI is number 1",
        env=env,
    )

    # After re-installing pypi-package, it should return "number 2"
    verify_cli_command([pixi, "reinstall", "--manifest-path", manifest, "pypi_package"], env=env)

    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "pypi-package-main"],
        stdout_contains="PyPI is number 2",
        env=env,
    )

    # After re-installing the whole default environment,
    # it should return "number 2"
    verify_cli_command([pixi, "reinstall", "--manifest-path", manifest], env=env)
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "pypi-package-main"],
        stdout_contains="PyPI is number 2",
        env=env,
    )


@pytest.mark.extra_slow
def test_pixi_reinstall_multi_env(pixi: Path, reinstall_workspace: Path) -> None:
    env = {
        "PIXI_CACHE_DIR": str(reinstall_workspace.joinpath("pixi_cache")),
    }
    manifest = reinstall_workspace.joinpath("pixi.toml")

    # Check that packages return "number 1" in default environment
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "pypi-package-main"],
        stdout_contains="PyPI is number 1",
        env=env,
    )

    # Check that packages return "number 1" in dev environment
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "--environment", "dev", "pypi-package-dev-main"],
        stdout_contains="PyPI dev is number 1",
        env=env,
    )

    # Modify the Python files
    for package in [
        "pypi_package",
        "pypi_package_dev",
    ]:
        init_py = reinstall_workspace.joinpath(package, "src", package, "__init__.py")
        init_py.write_text(init_py.read_text().replace("1", "2"))

    # That shouldn't trigger a re-install, so running still returns "number 1"
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "--environment", "dev", "pypi-package-dev-main"],
        stdout_contains="PyPI dev is number 1",
        env=env,
    )

    # After re-building pypi_package_dev, should still return "number 1"
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
        ],
        env=env,
    )
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "--environment", "dev", "pypi-package-dev-main"],
        stdout_contains="PyPI dev is number 2",
        env=env,
    )

    # In the default environment, it should still be "number 1"
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "pypi-package-main"],
        stdout_contains="PyPI is number 1",
        env=env,
    )

    # After reinstalling all environments,
    # also the default environment should be "number 2"
    verify_cli_command([pixi, "reinstall", "--manifest-path", manifest, "--all"], env=env)
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "pypi-package-main"],
        stdout_contains="PyPI is number 2",
        env=env,
    )
