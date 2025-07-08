# # import json
# # import os
# # import platform
# # import shutil
# # import tomllib
from pathlib import Path


from .common import (
    #     CURRENT_PLATFORM,
    #     EMPTY_BOILERPLATE_PROJECT,
    #     PIXI_VERSION,
    ExitCode,
    #     cwd,
    verify_cli_command,
    repo_root,
)


def test_import_invalid_format(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    # Create a new project
    verify_cli_command([pixi, "init", tmp_pixi_workspace])

    # Add package
    verify_cli_command(
        [
            pixi,
            "import",
            "--manifest-path",
            manifest_path,
            repo_root() / "tests/data/import_files/simple_environment.yml",
            "--format=foobar",
        ],
        ExitCode.FAILURE,
        stderr_contains="format",
    )


def test_import_conda_env(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    # Create a new project
    verify_cli_command([pixi, "init", tmp_pixi_workspace])

    # Add package
    verify_cli_command(
        [
            pixi,
            "import",
            "--manifest-path",
            manifest_path,
            repo_root() / "tests/data/import_files/simple_environment.yml",
            "--format=conda-env",
        ],
        ExitCode.SUCCESS,
        stderr_contains="Imported",
    )
