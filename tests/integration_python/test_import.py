from pathlib import Path

from .common import (
    ExitCode,
    verify_cli_command,
    repo_root,
)


class TestCondaEnv:
    def test_import_invalid_format(self, pixi: Path, tmp_pixi_workspace: Path) -> None:
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
            ExitCode.INCORRECT_USAGE,
            stderr_contains="invalid value 'foobar' for '--format <FORMAT>'",
        )

    def test_import_conda_env(self, pixi: Path, tmp_pixi_workspace: Path) -> None:
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
        )
        verify_cli_command(
            [pixi, "list", "--manifest-path", manifest_path, "--environment=simple-env"],
            stdout_contains="scipy",
        )

    def test_import_no_format(self, pixi: Path, tmp_pixi_workspace: Path) -> None:
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
        )
        verify_cli_command(
            [pixi, "list", "--manifest-path", manifest_path, "--environment=simple-env"],
            stdout_contains="scipy",
        )

    def test_import_no_name(self, pixi: Path, tmp_pixi_workspace: Path) -> None:
        manifest_path = tmp_pixi_workspace / "pixi.toml"
        # Create a new project
        verify_cli_command([pixi, "init", tmp_pixi_workspace])

        # Import an `environment.yml` without a name
        verify_cli_command(
            [
                pixi,
                "import",
                "--manifest-path",
                manifest_path,
                repo_root() / "tests/data/import_files/noname.yml",
            ],
            ExitCode.FAILURE,
            stderr_contains="Missing name: provide --feature or --environment, or set `name:`",
        )

        # Providing a feature name succeeds
        verify_cli_command(
            [
                pixi,
                "import",
                "--manifest-path",
                manifest_path,
                repo_root() / "tests/data/import_files/noname.yml",
                "--feature=foobar",
            ],
        )

    def test_import_platforms(self, pixi: Path, tmp_pixi_workspace: Path) -> None:
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

    def test_import_feature_environment(self, pixi: Path, tmp_pixi_workspace: Path) -> None:
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
            stderr_contains="Imported",
        )
        verify_cli_command(
            [pixi, "info", "--manifest-path", manifest_path],
            stdout_contains=["Environment: data", "Features: data"],
        )

    def test_import_channels_and_versions(self, pixi: Path, tmp_pixi_workspace: Path) -> None:
        manifest_path = tmp_pixi_workspace / "pixi.toml"
        # Create a new project
        verify_cli_command([pixi, "init", tmp_pixi_workspace])

        # Import an environment which uses bioconda, pins versions, and specifies a variant
        verify_cli_command(
            [
                pixi,
                "import",
                "--manifest-path",
                manifest_path,
                repo_root() / "tests/data/import_files/complex_environment.yml",
            ],
            stderr_contains="Imported",
        )
        verify_cli_command(
            [
                pixi,
                "list",
                "--manifest-path",
                manifest_path,
                "--environment=complex-env",
                "--explicit",
            ],
            stdout_contains=[
                "cowpy",
                "1.1.4",
                "libblas",
                "_openblas",
                "snakemake-minimal",
                "bioconda",
            ],
        )
