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


@pytest.mark.slow
def test_prefix_only_created_when_sdist(
    pixi: Path, tmp_pixi_workspace: Path, tmp_path: Path
) -> None:
    """Test that the prefix is only created when the source distribution is used"""
    test_data = Path(__file__).parent.parent / "data/pixi_tomls/two_envs_one_sdist.toml"
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = test_data.read_text()
    manifest.write_text(toml)

    # Install
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
        ExitCode.SUCCESS,
        # We need the an uncached version, otherwise it might skip prefix creation
        env={"PIXI_CACHE_DIR": str(tmp_path)},
    )

    # Test if the `py310` prefix is created but the `py39` is not
    py39 = tmp_pixi_workspace / ".pixi" / "envs" / "py39"
    py310 = tmp_pixi_workspace / ".pixi" / "envs" / "py310"

    assert not py39.exists()
    assert py310.exists()


def test_error_on_conda_meta_file_error(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    """Break the meta file and check if the error is caught and printed to the user"""
    verify_cli_command([pixi, "init", "-c", dummy_channel_1, tmp_pixi_workspace], ExitCode.SUCCESS)

    # Install a package
    verify_cli_command([pixi, "add", "dummy-a", "--manifest-path", tmp_pixi_workspace])

    # Create a conda meta file and path with an error
    meta_file = tmp_pixi_workspace.joinpath(
        ".pixi/envs/default/conda-meta/ca-certificates-2024.12.14-hf0a4a13_0.json"
    )
    meta_file.parent.mkdir(parents=True, exist_ok=True)
    meta_file.write_text("")

    verify_cli_command(
        [pixi, "install", "--manifest-path", tmp_pixi_workspace],
        ExitCode.FAILURE,
        stderr_contains=["failed to collect prefix records", "pixi clean"],
    )


def test_cuda_on_macos(pixi: Path, tmp_pixi_workspace: Path, virtual_packages_channel: str) -> None:
    """Test that we can install an environment that has cuda dependencies for linux on a macOS machine. This mimics the behavior of the pytorch installation where the linux environment should have cuda but the macOS environment should not."""
    verify_cli_command([pixi, "init", tmp_pixi_workspace, "--channel", virtual_packages_channel])
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    env = {"CONDA_OVERRIDE_CUDA": "12.0"}
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
    )

    # Add the dependency
    verify_cli_command(
        [pixi, "add", "--manifest-path", manifest, "noarch_package", "--no-install"],
        env=env,
    )

    # Install important to run on all platforms!
    # It should succeed even though we are on macOS
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
        env=env,
    )

    # Add the dependency even though the system requirements can not be satisfied on the machine
    verify_cli_command(
        [pixi, "add", "--manifest-path", manifest, "no-deps", "--no-install"],
        env=env,
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
