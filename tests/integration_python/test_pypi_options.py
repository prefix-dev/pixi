from pathlib import Path

import pytest
import yaml

from .common import CONDA_FORGE_CHANNEL, CURRENT_PLATFORM, ExitCode, verify_cli_command


@pytest.mark.slow
def test_no_build_option(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """
    Tests the behavior of pixi install command when the no-build option is specified in pixi.toml.
    This test verifies that the installation fails appropriately when attempting to install
    packages that need to be built like `sdist`.
    """
    test_data = Path(__file__).parent.parent / "data/pixi_tomls/no_build.toml"
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = test_data.read_text()
    manifest.write_text(toml)

    # Run the installation
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
        ExitCode.FAILURE,
    )


@pytest.mark.slow
def test_pypi_overrides(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """
    Tests the behavior of pixi install command with dependency overrides specified in pixi.toml.
    This test verifies that the installation succeeds when the overrides are correctly defined.
    """
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    manifest_content = f"""
    [workspace]
    channels = ["{CONDA_FORGE_CHANNEL}"]
    platforms = ["{CURRENT_PLATFORM}"]

    [dependencies]
    python = "3.13.*"

    [pypi-options.dependency-overrides]
    dummy_test = "==0.1.3"

    [pypi-dependencies]
    dummy_test = "==0.1.1"

    [feature.dev.pypi-options.dependency-overrides]
    dummy_test = "==0.1.2"

    [feature.outdated.pypi-options.dependency-overrides]
    dummy_test = "==0.1.0"

    [environments]
    dev = ["dev"]
    outdated = ["outdated"]
    """
    manifest.write_text(manifest_content)
    lock_file_path = tmp_pixi_workspace.joinpath("pixi.lock")

    # Run the installation
    verify_cli_command([pixi, "lock", "--manifest-path", manifest])

    with open(lock_file_path, "r") as f:
        lock = yaml.safe_load(f)

    assert any(
        "dummy_test-0.1.2" in v
        for pkg in lock["environments"]["dev"]["packages"][CURRENT_PLATFORM]
        for v in pkg.values()
    )
    assert any(
        "dummy_test-0.1.3" in v
        for pkg in lock["environments"]["default"]["packages"][CURRENT_PLATFORM]
        for v in pkg.values()
    )
    assert any(
        "dummy_test-0.1.0" in v
        for pkg in lock["environments"]["outdated"]["packages"][CURRENT_PLATFORM]
        for v in pkg.values()
    )


@pytest.mark.slow
def test_lock_no_install_with_dynamic_source_metadata(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """
    Locking a source dependency with dynamic metadata needs the conda environment installed,
    so `--no-install` should fail with a clear error and no panic message.
    """
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    manifest.write_text(f"""
    [workspace]
    channels = ["{CONDA_FORGE_CHANNEL}"]
    platforms = ["{CURRENT_PLATFORM}"]

    [dependencies]
    python = "3.13.*"

    [pypi-dependencies]
    dynpkg = {{ path = "./pkg", editable = true }}
    """)
    pkg = tmp_pixi_workspace.joinpath("pkg")
    pkg.joinpath("src", "dynpkg").mkdir(parents=True)
    pkg.joinpath("src", "dynpkg", "__init__.py").touch()
    pkg.joinpath("pyproject.toml").write_text("""
    [build-system]
    requires = ["setuptools"]
    build-backend = "setuptools.build_meta"

    [project]
    name = "dynpkg"
    dynamic = ["version", "dependencies"]
    """)
    pkg.joinpath("setup.py").write_text(
        'from setuptools import setup\nsetup(version="0.1", install_requires=[])\n'
    )

    verify_cli_command(
        [pixi, "lock", "--manifest-path", manifest, "--no-install"],
        ExitCode.FAILURE,
        stderr_contains="--no-install",
        stderr_excludes="panicked",
    )
