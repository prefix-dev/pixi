from pathlib import Path
import pytest
from .common import verify_cli_command, ExitCode


@pytest.mark.extra_slow
def test_no_build_option(pixi: Path, tmp_pixi_workspace: Path, tmp_path: Path) -> None:
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


def test_pypi_overrides(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """
    Tests the behavior of pixi install command with dependency overrides specified in pixi.toml.
    This test verifies that the installation succeeds when the overrides are correctly defined.
    """
    test_data = Path(__file__).parent.parent / "data/pixi_tomls/dependency_overrides.toml"
    # prepare if we would like to assert exactly same lock file
    # test_lock_data = Path(__file__).parent.parent / "data/lockfiles/dependency_overrides.lock"
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    lock_file_path = tmp_pixi_workspace.joinpath("pixi.lock")
    toml = test_data.read_text()
    manifest.write_text(toml)

    # Run the installation
    verify_cli_command([pixi, "install", "--manifest-path", manifest])
    lock_file_content = lock_file_path.read_text()

    # numpy 2.0.0 is overriding the dev env
    assert "numpy-2.0.0" in lock_file_content
    # numpy 1.21.0 is overriding the outdated env
    assert "numpy-1.21.0" in lock_file_content
