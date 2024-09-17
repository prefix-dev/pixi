from enum import IntEnum
from pathlib import Path
import subprocess
import tomllib
import tomli_w

PIXI_VERSION = "0.29.0"


class ExitCode(IntEnum):
    SUCCESS = 0
    FAILURE = 1
    INCORRECT_USAGE = 2


def verify_cli_command(
    command: list[Path | str],
    expected_exit_code: ExitCode,
    stdout_contains: str | list[str] | None = None,
    stdout_excludes: str | list[str] | None = None,
    stderr_contains: str | list[str] | None = None,
    stderr_excludes: str | list[str] | None = None,
    env: dict[str, str] | None = None,
) -> None:
    process = subprocess.run(command, capture_output=True, text=True, env=env)
    stdout, stderr, returncode = process.stdout, process.stderr, process.returncode
    print(f"command: {command}, stdout: {stdout}, stderr: {stderr}, code: {returncode}")
    if expected_exit_code is not None:
        assert (
            returncode == expected_exit_code
        ), f"Return code was {returncode}, expected {expected_exit_code}, stderr: {stderr}"

    if stdout_contains:
        if isinstance(stdout_contains, str):
            stdout_contains = [stdout_contains]
        for substring in stdout_contains:
            assert substring in stdout, f"'{substring}' not found in stdout: {stdout}"

    if stdout_excludes:
        if isinstance(stdout_excludes, str):
            stdout_excludes = [stdout_excludes]
        for substring in stdout_excludes:
            assert substring not in stdout, f"'{substring}' unexpectedly found in stdout: {stdout}"

    if stderr_contains:
        if isinstance(stderr_contains, str):
            stderr_contains = [stderr_contains]
        for substring in stderr_contains:
            assert substring in stderr, f"'{substring}' not found in stderr: {stderr}"

    if stderr_excludes:
        if isinstance(stderr_excludes, str):
            stderr_excludes = [stderr_excludes]
        for substring in stderr_excludes:
            assert substring not in stderr, f"'{substring}' unexpectedly found in stderr: {stderr}"


def test_pixi(pixi: Path) -> None:
    verify_cli_command(
        [pixi], ExitCode.INCORRECT_USAGE, stdout_excludes=f"[version {PIXI_VERSION}]"
    )
    verify_cli_command([pixi, "--version"], ExitCode.SUCCESS, stdout_contains=PIXI_VERSION)


def test_project_commands(pixi: Path, tmp_path: Path) -> None:
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


def test_global_sync_dependencies(pixi: Path, tmp_path: Path) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    toml = """
    [envs.test]
    channels = ["conda-forge"]
    [envs.test.dependencies]
    python = "3.12"

    [envs.test.exposed]
    "python-injected" = "python"
    """
    parsed_toml = tomllib.loads(toml)
    manifest.write_text(toml)
    python_injected = tmp_path / "bin" / "python-injected"

    # Test basic commands
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=env)
    verify_cli_command(
        [python_injected, "--version"], ExitCode.SUCCESS, env=env, stdout_contains="3.12"
    )
    verify_cli_command([python_injected, "-c", "import numpy"], ExitCode.FAILURE, env=env)

    # Add numpy
    parsed_toml["envs"]["test"]["dependencies"]["numpy"] = "*"
    manifest.write_text(tomli_w.dumps(parsed_toml))
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=env)
    verify_cli_command([python_injected, "-c", "import numpy"], ExitCode.SUCCESS, env=env)

    # Remove numpy again
    del parsed_toml["envs"]["test"]["dependencies"]["numpy"]
    manifest.write_text(tomli_w.dumps(parsed_toml))
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=env)
    verify_cli_command([python_injected, "-c", "import numpy"], ExitCode.FAILURE, env=env)

    # Remove python
    del parsed_toml["envs"]["test"]["dependencies"]["python"]
    manifest.write_text(tomli_w.dumps(parsed_toml))
    verify_cli_command(
        [pixi, "global", "sync"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains="Could not find python in test",
    )


def test_global_sync_channels(pixi: Path, tmp_path: Path) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    toml = """
    [envs.test]
    channels = ["conda-forge"]
    [envs.test.dependencies]
    python = "*"
    bionumpy = "*"

    [envs.test.exposed]
    "python-injected" = "python"
    """
    parsed_toml = tomllib.loads(toml)
    manifest.write_text(toml)
    python_injected = tmp_path / "bin" / "python-injected"

    # Test basic commands
    verify_cli_command(
        [pixi, "global", "sync"], ExitCode.FAILURE, env=env, stderr_contains="bionumpy"
    )

    # Add bioconda channel
    parsed_toml["envs"]["test"]["channels"].append("bioconda")
    manifest.write_text(tomli_w.dumps(parsed_toml))
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=env)
    verify_cli_command([python_injected, "-c", "import bionumpy"], ExitCode.SUCCESS, env=env)


def test_global_sync_platform(pixi: Path, tmp_path: Path) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    toml = """
    [envs.test]
    channels = ["conda-forge"]
    platform = "linux-64"
    [envs.test.dependencies]
    binutils = "2.40"
    [envs.test.exposed]
    """
    parsed_toml = tomllib.loads(toml)
    manifest.write_text(toml)
    # Exists on linux-64
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=env)

    # Also exists on win-64
    parsed_toml["envs"]["test"]["platform"] = "win-64"
    manifest.write_text(tomli_w.dumps(parsed_toml))
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=env)

    # Does not exist on osx-64
    parsed_toml["envs"]["test"]["platform"] = "osx-64"
    manifest.write_text(tomli_w.dumps(parsed_toml))
    verify_cli_command([pixi, "global", "sync"], ExitCode.FAILURE, env=env)


def test_global_sync_change_expose(pixi: Path, tmp_path: Path) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    toml = """
    [envs.test]
    channels = ["conda-forge"]
    [envs.test.dependencies]
    python = "3.12"

    [envs.test.exposed]
    "python-injected" = "python"
    """
    parsed_toml = tomllib.loads(toml)
    manifest.write_text(toml)
    python_injected = tmp_path / "bin" / "python-injected"

    # Test basic commands
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=env)
    verify_cli_command(
        [python_injected, "--version"], ExitCode.SUCCESS, env=env, stdout_contains="3.12"
    )
    verify_cli_command([python_injected], ExitCode.SUCCESS, env=env)

    # Add another expose
    python_in_disguise_str = "python-in-disguise"
    python_in_disguise = tmp_path / "bin" / python_in_disguise_str
    parsed_toml["envs"]["test"]["exposed"][python_in_disguise_str] = "python"
    manifest.write_text(tomli_w.dumps(parsed_toml))
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=env)
    verify_cli_command([python_in_disguise], ExitCode.SUCCESS, env=env)

    # Remove expose again
    del parsed_toml["envs"]["test"]["exposed"][python_in_disguise_str]
    manifest.write_text(tomli_w.dumps(parsed_toml))
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=env)
    assert not python_in_disguise.is_file()


def test_global_sync_manually_remove_binary(pixi: Path, tmp_path: Path) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    toml = """
    [envs.test]
    channels = ["conda-forge"]
    [envs.test.dependencies]
    python = "3.12"

    [envs.test.exposed]
    "python-injected" = "python"
    """
    manifest.write_text(toml)
    python_injected = tmp_path / "bin" / "python-injected"

    # Test basic commands
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=env)
    verify_cli_command(
        [python_injected, "--version"], ExitCode.SUCCESS, env=env, stdout_contains="3.12"
    )
    verify_cli_command([python_injected], ExitCode.SUCCESS, env=env)

    # Remove binary manually
    python_injected.unlink()

    # Binary is added again
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=env)
    verify_cli_command(
        [python_injected, "--version"], ExitCode.SUCCESS, env=env, stdout_contains="3.12"
    )


def test_global_sync_migrate(pixi: Path, tmp_path: Path) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    toml = """
    [envs.test]
    channels = ["https://conda.anaconda.org/conda-forge"]
    [envs.test.dependencies]
    ripgrep = "*"
    python = "*"

    [envs.test.exposed]
    rg = "rg"
    grep = "rg"
    python = "python3"
    python3 = "python"
    """
    manifest.write_text(toml)
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=env)

    # Test migration from existing environments
    original_manifest = tomllib.loads(manifest.read_text())
    manifest.unlink()
    verify_cli_command([pixi, "global", "sync", "--assume-yes"], ExitCode.SUCCESS, env=env)
    migrated_manifest = tomllib.loads(manifest.read_text())
    assert original_manifest == migrated_manifest


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


def test_simple_project_setup(pixi: Path, tmp_path: Path) -> None:
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


def test_pixi_init_pyproject(pixi: Path, tmp_path: Path) -> None:
    manifest_path = tmp_path / "pyproject.toml"
    # Create a new project
    verify_cli_command([pixi, "init", tmp_path, "--format", "pyproject"], ExitCode.SUCCESS)
    # Verify that install works
    verify_cli_command([pixi, "install", "--manifest-path", manifest_path], ExitCode.SUCCESS)
