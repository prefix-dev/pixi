from pathlib import Path

from .common import verify_cli_command, ALL_PLATFORMS


def test_variable_expansion(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = f"""
        [workspace]
        channels = ["conda-forge"]
        name = "expansion-test"
        platforms = {ALL_PLATFORMS}
        version = "0.1.0"

        [activation.env]
        TEST_VAR = "$PIXI_PROJECT_NAME"

        [tasks]
        start = "echo The project name is $TEST_VAR"
        """
    manifest.write_text(toml)

    # If variable expansion works, we expect `$PIXI_PROJECT_NAME` to expand to the project name "expansion-test"
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "start"],
        stdout_contains="The project name is expansion-test",
    )
