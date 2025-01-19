from pathlib import Path
import pytest
import platform
from .common import verify_cli_command, ExitCode


@pytest.mark.extra_slow
def test_pypi_git_deps(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Test where we need to lookup recursive git dependencies and consider them first party"""
    test_data = Path(__file__).parent.parent / "data/pixi_tomls/pip_git_dep.toml"
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = test_data.read_text()
    manifest.write_text(toml)

    # Run the installation
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
        ExitCode.SUCCESS,
    )


@pytest.mark.slow
@pytest.mark.skipif(
    not (platform.system() == "Darwin" and platform.machine() == "arm64"),
    reason="Test tailored for macOS arm so that we can get two different python interpreters",
)
def test_python_mismatch(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Test pypi wheel install where the base interpreter is not the same as the target version"""
    test_data = Path(__file__).parent.parent / "data/pixi_tomls/python_mismatch.toml"
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = test_data.read_text()
    manifest.write_text(toml)

    # Install
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
        ExitCode.SUCCESS,
    )


def test_cuda_on_macos(pixi: Path, tmp_pixi_workspace: Path, virtual_packages_channel: str) -> None:
    """Test that we can install an environment that has cuda dependencies for linux on a macOS machine. This mimics the behavior of the pytorch installation where the linux environment should have cuda but the macOS environment should not."""
    verify_cli_command([pixi, "init", tmp_pixi_workspace, "--channel", virtual_packages_channel])
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")

    # Add multiple platforms
    verify_cli_command(
        [
            pixi,
            "project",
            "platform",
            "add",
            "--manifest-path",
            manifest,
            "linux-64",
            "osx-64",
            "osx-arm64",
            "win-64",
        ],
        ExitCode.SUCCESS,
    )

    # Add system-requirement on cuda
    verify_cli_command(
        [
            pixi,
            "project",
            "system-requirements",
            "add",
            "--manifest-path",
            manifest,
            "cuda",
            "12.1",
        ],
        ExitCode.SUCCESS,
    )

    # Add the dependency
    verify_cli_command(
        [pixi, "add", "--manifest-path", manifest, "noarch_package", "--no-install"],
        ExitCode.SUCCESS,
    )

    # Install important to run on all platforms!
    # It should succeed even though we are on macOS
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
        ExitCode.SUCCESS,
    )

    # Add the dependency even though the system requirements can not be satisfied on the machine
    verify_cli_command(
        [pixi, "add", "--manifest-path", manifest, "no-deps", "--no-install"],
        ExitCode.SUCCESS,
    )


def test_unused_strict_system_requirements(
    pixi: Path, tmp_pixi_workspace: Path, virtual_packages_channel: str
) -> None:
    """Setup a project with strict system requirements that are not used by any package"""
    verify_cli_command([pixi, "init", tmp_pixi_workspace, "--channel", virtual_packages_channel])
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")

    # Add system-requirement on cuda
    verify_cli_command(
        [
            pixi,
            "project",
            "system-requirements",
            "add",
            "--manifest-path",
            manifest,
            "cuda",
            "12.1",
        ],
        ExitCode.SUCCESS,
    )
    # Add non existing glibc
    verify_cli_command(
        [
            pixi,
            "project",
            "system-requirements",
            "add",
            "--manifest-path",
            manifest,
            "glibc",
            "100.2.3",
        ],
        ExitCode.SUCCESS,
    )

    # Add non existing macos
    verify_cli_command(
        [
            pixi,
            "project",
            "system-requirements",
            "add",
            "--manifest-path",
            manifest,
            "macos",
            "123.123.0",
        ],
        ExitCode.SUCCESS,
    )

    # Add the dependency even though the system requirements can not be satisfied on the machine
    verify_cli_command(
        [pixi, "add", "--manifest-path", manifest, "no-deps", "--no-install"],
        ExitCode.SUCCESS,
    )

    # Installing should succeed as there is no virtual package that requires the system requirements
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
        ExitCode.SUCCESS,
    )

    # Activate the environment even though the machine doesn't have the system requirements
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "echo", "Hello World"],
        ExitCode.SUCCESS,
    )
