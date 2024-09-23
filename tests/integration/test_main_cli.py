from pathlib import Path

from .common import verify_cli_command, ExitCode, PIXI_VERSION


def test_pixi(pixi: Path) -> None:
    verify_cli_command(
        [pixi], ExitCode.INCORRECT_USAGE, stdout_excludes=f"[version {PIXI_VERSION}]"
    )
    verify_cli_command([pixi, "--version"], ExitCode.SUCCESS, stdout_contains=PIXI_VERSION)


def test_project_commands(tmp_path: Path, pixi: Path) -> None:
    manifest_path = tmp_path / "pixi.toml"
    # Create a new project
    verify_cli_command([pixi, "init", tmp_path], ExitCode.SUCCESS)

    # Channel commands
    verify_cli_command(
        [
            pixi,
            "project",
            "--manifest-path",
            manifest_path,
            "channel",
            "add",
            "bioconda",
        ],
        ExitCode.SUCCESS,
    )
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "channel", "list"],
        ExitCode.SUCCESS,
        stdout_contains="bioconda",
    )
    verify_cli_command(
        [
            pixi,
            "project",
            "--manifest-path",
            manifest_path,
            "channel",
            "remove",
            "bioconda",
        ],
        ExitCode.SUCCESS,
    )

    # Description commands
    verify_cli_command(
        [
            pixi,
            "project",
            "--manifest-path",
            manifest_path,
            "description",
            "set",
            "blabla",
        ],
        ExitCode.SUCCESS,
    )
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "description", "get"],
        ExitCode.SUCCESS,
        stdout_contains="blabla",
    )

    # Environment commands
    verify_cli_command(
        [
            pixi,
            "project",
            "--manifest-path",
            manifest_path,
            "environment",
            "add",
            "test",
        ],
        ExitCode.SUCCESS,
    )
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "environment", "list"],
        ExitCode.SUCCESS,
        stdout_contains="test",
    )
    verify_cli_command(
        [
            pixi,
            "project",
            "--manifest-path",
            manifest_path,
            "environment",
            "remove",
            "test",
        ],
        ExitCode.SUCCESS,
    )
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "environment", "list"],
        ExitCode.SUCCESS,
        stdout_excludes="test",
    )

    # Platform commands
    verify_cli_command(
        [
            pixi,
            "project",
            "--manifest-path",
            manifest_path,
            "platform",
            "add",
            "linux-64",
        ],
        ExitCode.SUCCESS,
    )
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "platform", "list"],
        ExitCode.SUCCESS,
        stdout_contains="linux-64",
    )
    verify_cli_command(
        [
            pixi,
            "project",
            "--manifest-path",
            manifest_path,
            "platform",
            "remove",
            "linux-64",
        ],
        ExitCode.SUCCESS,
    )
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "platform", "list"],
        ExitCode.SUCCESS,
        stdout_excludes="linux-64",
    )

    # Version commands
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "version", "set", "1.2.3"],
        ExitCode.SUCCESS,
    )
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "version", "get"],
        ExitCode.SUCCESS,
        stdout_contains="1.2.3",
    )
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "version", "major"],
        ExitCode.SUCCESS,
        stderr_contains="2.2.3",
    )
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "version", "minor"],
        ExitCode.SUCCESS,
        stderr_contains="2.3.3",
    )
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "version", "patch"],
        ExitCode.SUCCESS,
        stderr_contains="2.3.4",
    )


def test_global_install(pixi: Path) -> None:
    # Install
    verify_cli_command(
        [pixi, "global", "install", "rattler-build"],
        ExitCode.SUCCESS,
        stdout_excludes="rattler-build",
    )

    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "rattler-build",
            "-c",
            "https://fast.prefix.dev/conda-forge",
        ],
        ExitCode.SUCCESS,
        stdout_excludes="rattler-build",
    )

    # Install with --no-activation
    verify_cli_command(
        [pixi, "global", "install", "rattler-build", "--no-activation"],
        ExitCode.SUCCESS,
        stdout_excludes="rattler-build",
    )

    # Upgrade
    verify_cli_command([pixi, "global", "upgrade", "rattler-build"], ExitCode.SUCCESS)

    # Upgrade all
    verify_cli_command([pixi, "global", "upgrade-all"], ExitCode.SUCCESS)

    # List
    verify_cli_command([pixi, "global", "list"], ExitCode.SUCCESS, stderr_contains="rattler-build")

    # Remove
    verify_cli_command([pixi, "global", "remove", "rattler-build"], ExitCode.SUCCESS)
    verify_cli_command([pixi, "global", "remove", "rattler-build"], ExitCode.FAILURE)


def test_search(pixi: Path) -> None:
    verify_cli_command(
        [pixi, "search", "rattler-build", "-c", "conda-forge"],
        ExitCode.SUCCESS,
        stdout_contains="rattler-build",
    )
    verify_cli_command(
        [pixi, "search", "rattler-build", "-c", "https://fast.prefix.dev/conda-forge"],
        ExitCode.SUCCESS,
        stdout_contains="rattler-build",
    )


def test_simple_project_setup(tmp_path: Path, pixi: Path) -> None:
    manifest_path = tmp_path / "pixi.toml"
    # Create a new project
    verify_cli_command([pixi, "init", tmp_path], ExitCode.SUCCESS)

    # Add package
    verify_cli_command(
        [pixi, "add", "--manifest-path", manifest_path, "_r-mutex"],
        ExitCode.SUCCESS,
        stderr_contains="Added",
    )
    verify_cli_command(
        [
            pixi,
            "add",
            "--manifest-path",
            manifest_path,
            "--feature",
            "test",
            "_r-mutex==1.0.1",
        ],
        ExitCode.SUCCESS,
        stderr_contains=["test", "==1.0.1"],
    )
    verify_cli_command(
        [
            pixi,
            "add",
            "--manifest-path",
            manifest_path,
            "--platform",
            "linux-64",
            "conda-forge::_r-mutex",
        ],
        ExitCode.SUCCESS,
        stderr_contains=["linux-64", "conda-forge"],
    )
    verify_cli_command(
        [
            pixi,
            "add",
            "--manifest-path",
            manifest_path,
            "-f",
            "test",
            "-p",
            "osx-arm64",
            "_r-mutex",
        ],
        ExitCode.SUCCESS,
        stderr_contains=["osx-arm64", "test"],
    )

    # Remove package
    verify_cli_command(
        [pixi, "remove", "--manifest-path", manifest_path, "_r-mutex"],
        ExitCode.SUCCESS,
        stderr_contains="Removed",
    )
    verify_cli_command(
        [
            pixi,
            "remove",
            "--manifest-path",
            manifest_path,
            "--feature",
            "test",
            "_r-mutex",
        ],
        ExitCode.SUCCESS,
        stderr_contains=["test", "Removed"],
    )
    verify_cli_command(
        [
            pixi,
            "remove",
            "--manifest-path",
            manifest_path,
            "--platform",
            "linux-64",
            "conda-forge::_r-mutex",
        ],
        ExitCode.SUCCESS,
        stderr_contains=["linux-64", "conda-forge", "Removed"],
    )
    verify_cli_command(
        [
            pixi,
            "remove",
            "--manifest-path",
            manifest_path,
            "-f",
            "test",
            "-p",
            "osx-arm64",
            "_r-mutex",
        ],
        ExitCode.SUCCESS,
        stderr_contains=["osx-arm64", "test", "Removed"],
    )


def test_pixi_init_pyproject(tmp_path: Path, pixi: Path) -> None:
    manifest_path = tmp_path / "pyproject.toml"
    # Create a new project
    verify_cli_command([pixi, "init", tmp_path, "--format", "pyproject"], ExitCode.SUCCESS)
    # Verify that install works
    verify_cli_command([pixi, "install", "--manifest-path", manifest_path], ExitCode.SUCCESS)
