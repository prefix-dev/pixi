from pathlib import Path

from .common import (
    ExitCode,
    verify_cli_command,
    repo_root,
)


def test_import_invalid_format(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    # Create a new project
    verify_cli_command([pixi, "init", tmp_pixi_workspace])

    # try to import as an invalid format
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

    # Import a simple `environment.yml`
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
    verify_cli_command(
        [pixi, "list", "--manifest-path", manifest_path, "--environment=simple-env"],
        stdout_contains="scipy",
    )


def test_import_no_format(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    # Create a new project
    verify_cli_command([pixi, "init", tmp_pixi_workspace])

    # Import a simple `environment.yml` without specifying `format`
    verify_cli_command(
        [
            pixi,
            "import",
            "--manifest-path",
            manifest_path,
            repo_root() / "tests/data/import_files/simple_environment.yml",
        ],
        ExitCode.SUCCESS,
        stderr_contains="Imported",
    )
    verify_cli_command(
        [pixi, "list", "--manifest-path", manifest_path, "--environment=simple-env"],
        stdout_contains="scipy",
    )


def test_import_platforms(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    # Create a new project
    verify_cli_command([pixi, "init", tmp_pixi_workspace])

    # Import a simple `environment.yml` for linux-64 only
    verify_cli_command(
        [
            pixi,
            "import",
            "--manifest-path",
            manifest_path,
            repo_root() / "tests/data/import_files/simple_environment.yml",
            "--platform=linux-64",
        ],
        ExitCode.SUCCESS,
        stderr_contains="Imported",
    )
    verify_cli_command(
        [
            pixi,
            "list",
            "--manifest-path",
            manifest_path,
            "--environment=simple-env",
            "--platform=linux-64",
        ],
        stdout_contains="scipy",
    )
    verify_cli_command(
        [
            pixi,
            "list",
            "--manifest-path",
            manifest_path,
            "--environment=simple-env",
            "--platform=osx-arm64",
        ],
        ExitCode.FAILURE,
        stderr_contains="platform",
    )


def test_import_feature_environment(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    # Create a new project
    verify_cli_command([pixi, "init", tmp_pixi_workspace])

    # by default, a new env and feature are created with the name of the imported file
    verify_cli_command(
        [
            pixi,
            "import",
            "--manifest-path",
            manifest_path,
            repo_root() / "tests/data/import_files/simple_environment.yml",
        ],
        ExitCode.SUCCESS,
        stderr_contains="Imported",
    )
    verify_cli_command(
        [pixi, "info", "--manifest-path", manifest_path],
        stdout_contains=["Environment: simple-env", "Features: simple-env"],
    )

    # we can import into an existing feature
    verify_cli_command(
        [
            pixi,
            "import",
            "--manifest-path",
            manifest_path,
            repo_root() / "tests/data/import_files/cowpy.yml",
            "--feature=simple-env",
        ],
        ExitCode.SUCCESS,
        stderr_contains="Imported",
    )
    verify_cli_command(
        [pixi, "list", "--manifest-path", manifest_path, "--environment=simple-env"],
        stdout_contains=["cowpy"],
    )
    # no new environment should be created
    verify_cli_command(
        [pixi, "list", "--manifest-path", manifest_path, "--environment=cowpy"],
        ExitCode.FAILURE,
        stderr_contains=["unknown environment"],
    )

    # we can create a new feature and add it to an existing environment
    verify_cli_command(
        [
            pixi,
            "import",
            "--manifest-path",
            manifest_path,
            repo_root() / "tests/data/import_files/array-api-extra.yml",
            "--environment=simple-env",
            "--feature=array-api-extra",
        ],
        ExitCode.SUCCESS,
        stderr_contains="Imported",
    )
    verify_cli_command(
        [pixi, "info", "--manifest-path", manifest_path],
        stdout_contains=["Environment: simple-env", "Features: simple-env, array-api-extra"],
    )
    # no new environment should be created
    verify_cli_command(
        [pixi, "list", "--manifest-path", manifest_path, "--environment=array-api-extra"],
        ExitCode.FAILURE,
        stderr_contains=["unknown environment"],
    )

    # we can create a new feature (and a matching env by default)
    verify_cli_command(
        [
            pixi,
            "import",
            "--manifest-path",
            manifest_path,
            repo_root() / "tests/data/import_files/cowpy.yml",
            "--feature=farm",
        ],
        ExitCode.SUCCESS,
        stderr_contains="Imported",
    )
    verify_cli_command(
        [pixi, "info", "--manifest-path", manifest_path],
        stdout_contains=["Environment: farm", "Features: farm"],
    )

    # we can create a new env (and a matching feature by default)
    verify_cli_command(
        [
            pixi,
            "import",
            "--manifest-path",
            manifest_path,
            repo_root() / "tests/data/import_files/array-api-extra.yml",
            "--feature=data",
        ],
        ExitCode.SUCCESS,
        stderr_contains="Imported",
    )
    verify_cli_command(
        [pixi, "info", "--manifest-path", manifest_path],
        stdout_contains=["Environment: data", "Features: data"],
    )
