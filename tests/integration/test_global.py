from pathlib import Path
import tomllib
import tomli_w
from .common import verify_cli_command, ExitCode
import platform


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
    exposed_exec = "python-injected.bat" if platform.system() == "Windows" else "python-injected"
    python_injected = tmp_path / "bin" / exposed_exec

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


def test_global_sync_channels(pixi: Path, tmp_path: Path, test_data: Path) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    toml = """
    [envs.test]
    channels = ["conda-forge"]
    [envs.test.dependencies]
    python = "*"
    life_package = "*"

    [envs.test.exposed]
    life = "life"
    """
    parsed_toml = tomllib.loads(toml)
    manifest.write_text(toml)

    # Test basic commands
    verify_cli_command([pixi, "global", "sync"], ExitCode.FAILURE, env=env, stderr_contains="life")

    # Add bioconda channel
    simple_channel = (test_data / "simple" / "output").as_uri()
    parsed_toml["envs"]["test"]["channels"].append(simple_channel)
    manifest.write_text(tomli_w.dumps(parsed_toml))
    life = tmp_path / "bin" / ("life.bat" if platform.system() == "Windows" else "life")
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=env)
    verify_cli_command([life], ExitCode.LIFE, env=env)


def test_global_sync_platform(pixi: Path, tmp_path: Path) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    toml = """
    [envs.test]
    channels = ["conda-forge"]
    platform = "win-64"
    [envs.test.dependencies]
    binutils = "2.40"
    [envs.test.exposed]
    """
    parsed_toml = tomllib.loads(toml)
    manifest.write_text(toml)
    # Exists on win-64
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=env)

    # Does not exist on osx-64
    parsed_toml["envs"]["test"]["platform"] = "osx-64"
    manifest.write_text(tomli_w.dumps(parsed_toml))
    verify_cli_command(
        [pixi, "global", "sync"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains="No candidates were found",
    )


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
    exposed_exec = "python-injected.bat" if platform.system() == "Windows" else "python-injected"
    python_injected = tmp_path / "bin" / exposed_exec

    # Test basic commands
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=env)
    verify_cli_command(
        [python_injected, "--version"], ExitCode.SUCCESS, env=env, stdout_contains="3.12"
    )
    verify_cli_command([python_injected], ExitCode.SUCCESS, env=env)

    # Add another expose
    python_in_disguise_str = (
        "python-in-disguise.bat" if platform.system() == "Windows" else "python-in-disguise"
    )
    python_in_disguise = tmp_path / "bin" / python_in_disguise_str
    parsed_toml["envs"]["test"]["exposed"][python_in_disguise_str] = "python"
    manifest.write_text(tomli_w.dumps(parsed_toml))
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=env)
    verify_cli_command([python_in_disguise, "--version"], ExitCode.SUCCESS, env=env)

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
    exposed_exec = "python-injected.bat" if platform.system() == "Windows" else "python-injected"
    python_injected = tmp_path / "bin" / exposed_exec

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
    python = "python"
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
