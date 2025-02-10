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
        env={"PIXI_CACHE_DIR": str(tmp_path)},
    )
