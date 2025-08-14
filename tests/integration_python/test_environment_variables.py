from pathlib import Path

from .common import ALL_PLATFORMS, verify_cli_command


def test_variable_expansion(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    var_pattern = "${BAR}"
    toml = f"""
        [workspace]
        channels = ["conda-forge"]
        name = "expansion-test"
        platforms = {ALL_PLATFORMS}
        version = "0.1.0"

        [activation.env]
        TEST_VAR = "$PIXI_PROJECT_NAME"
        BAR = "456"
        ANOTHER_VAR = "{var_pattern}"
        TODAY="$(date)"

        [target.win-64.activation.env]
        TEST_VAR = "%PIXI_PROJECT_NAME%"
        TODAY="$(Get-Date)"

        [tasks]
        start = "echo The project name is $TEST_VAR"
        today = "echo Today is: $TODAY"

        [tasks.foo]
        cmd = "echo $FOO"

        [tasks.foo.env]
        MY_FOO = "123"
        FOO = "$MY_FOO"
        """
    manifest.write_text(toml)

    # Test variable expansion schema `$VAR` for activation.env
    # If variable expansion works, we expect `$PIXI_PROJECT_NAME` to expand to the project name "expansion-test"
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "start"],
        stdout_contains="The project name is expansion-test",
    )

    # Test variable expansion schema `$VAR` for task.env
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "foo"],
        stdout_contains="123",
        stderr_excludes="$MY_FOO",
    )

    # Test command substitution schema `$(command)` for activation.env
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "today"],
        stdout_contains="Today is:",
        stderr_excludes=["$(date)", "$(Get-Date)"],
    )
