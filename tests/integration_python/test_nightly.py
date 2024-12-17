# These are test that are to be run nightly as they are very slow.
# They are not part of the normal test suite.
from pathlib import Path

from .common import verify_cli_command, ExitCode, root, current_platform
import pytest


@pytest.mark.slow
@pytest.mark.parametrize(
    "manifest",
    [
        pytest.param(manifest, id=manifest.stem)
        for manifest in root().joinpath("docs/source_files/").glob("**/pytorch-*.toml")
    ],
)
def test_pytorch_documentation_examples(
    pixi: Path, tmp_pixi_workspace: Path, manifest: Path
) -> None:
    # Copy the manifest to the tmp workspace
    toml = manifest.read_text()
    toml_name = "pyproject.toml" if "pyproject_tomls" in str(manifest) else "pixi.toml"
    manifest = tmp_pixi_workspace.joinpath(toml_name)
    manifest.write_text(toml)

    # Only solve if the platform is supported
    if (
        current_platform()
        in verify_cli_command(
            [pixi, "project", "platform", "ls", "--manifest-path", manifest],
            ExitCode.SUCCESS,
        ).stdout
    ):
        # Run the installation
        verify_cli_command(
            [pixi, "install", "--manifest-path", manifest],
            ExitCode.SUCCESS,
        )
