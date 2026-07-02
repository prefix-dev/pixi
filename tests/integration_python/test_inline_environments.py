from pathlib import Path

from .common import CURRENT_PLATFORM, ExitCode, verify_cli_command


def test_inline_environment_installs_and_runs(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = f"""
    [workspace]
    name = "test"
    channels = ["{dummy_channel_1}"]
    platforms = ["{CURRENT_PLATFORM}"]

    [environments.dev]
    dependencies = {{ dummy-a = "*" }}

    [environments.dev.tasks]
    greet = "echo hello-from-dev"
    """
    manifest.write_text(toml)

    # The inline task runs in the inline environment.
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "--environment", "dev", "greet"],
        stdout_contains="hello-from-dev",
    )

    # The inline dependency is installed in the environment.
    verify_cli_command(
        [pixi, "list", "--manifest-path", manifest, "--environment", "dev"],
        stdout_contains="dummy-a",
    )


def test_inline_environment_combines_with_features(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = f"""
    [workspace]
    name = "test"
    channels = ["{dummy_channel_1}"]
    platforms = ["{CURRENT_PLATFORM}"]

    [feature.shared.dependencies]
    dummy-b = "*"

    [environments.dev]
    features = ["shared"]
    dependencies = {{ dummy-a = "*" }}
    """
    manifest.write_text(toml)

    verify_cli_command(
        [pixi, "list", "--manifest-path", manifest, "--environment", "dev"],
        stdout_contains=["dummy-a", "dummy-b"],
    )


def test_reserved_env_feature_name_rejected(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = f"""
    [workspace]
    name = "test"
    channels = ["{dummy_channel_1}"]
    platforms = ["{CURRENT_PLATFORM}"]

    [environments.dev]
    dependencies = {{ dummy-a = "*" }}
    """
    manifest.write_text(toml)

    # The reserved feature namespace is rejected on the CLI.
    verify_cli_command(
        [pixi, "add", "--manifest-path", manifest, "--feature", "env:dev", "dummy-b"],
        ExitCode.INCORRECT_USAGE,
        stderr_contains="reserved",
    )

    # Referencing the synthesized feature of another environment is rejected.
    toml += """
    [environments.other]
    features = ["env:dev"]
    """
    manifest.write_text(toml)
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest, "--environment", "other"],
        ExitCode.FAILURE,
        stderr_contains="cannot be referenced",
    )
