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


@pytest.fixture
def cpp_simple(tmp_pixi_workspace: Path, test_data: Path) -> Path:
    test_rebuild_src = test_data / "cpp_simple"
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


@pytest.mark.extra_slow
def test_hidden_folder_dont_rebuild(pixi: Path, cpp_simple: Path) -> None:
    env = {
        "PIXI_CACHE_DIR": str(cpp_simple.joinpath("pixi_cache")),
    }
    manifest = cpp_simple.joinpath("pixi.toml")

    # we should see a build on a first install
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
        stderr_contains="Running build for recipe",
        env=env,
    )

    # Adding an empty file in the .pixi folder should not trigger a rebuild
    temp_file = cpp_simple.joinpath(".pixi", "SHOULD_NOT_TRIGGER_REBUILD")
    temp_file.write_text("This file should not trigger a rebuild")
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
        stdout_excludes="Running build for recipe",
    )

    # now we add some empty lines in the src/main.cpp file, which should trigger a rebuild
    main_cpp = cpp_simple.joinpath("src", "main.cc")
    main_cpp.write_text(main_cpp.read_text() + "\n\n\n")
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
        stderr_contains="Running build for recipe",
    )
