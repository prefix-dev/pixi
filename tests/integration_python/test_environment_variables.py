from pathlib import Path

from .common import verify_cli_command


def test_variable_expansion(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = """
        [workspace]
        channels = ["conda-forge"]
        name = "expansion-test"
        platforms = ["linux-64"]
        version = "0.1.0"

        [activation.env]
        TEST_VAR = "$PIXI_PROJECT_NAME"

        [tasks]
        start = "echo $TEST_VAR"
        """
    manifest.write_text(toml)

    # If variable expansion works, we expect `$PIXI_PROJECT_NAME` to expand to the project name "expansion-test"
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "start"], stdout_contains="expansion-test"
    )
