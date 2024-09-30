from pathlib import Path
import tomllib
import tomli_w
from .common import verify_cli_command, ExitCode
import platform


def exec_extension(exe_name: str) -> str:
    if platform.system() == "Windows":
        return exe_name + ".bat"
    else:
        return exe_name


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
    python_injected = tmp_path / "bin" / exec_extension("python-injected")

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


def test_global_sync_change_expose(pixi: Path, tmp_path: Path, test_data: Path) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    dummy_channel = test_data.joinpath("dummy_channel_1/output").as_uri()
    toml = f"""
    [envs.test]
    channels = ["{dummy_channel}"]
    [envs.test.dependencies]
    dummy-a = "*"

    [envs.test.exposed]
    "dummy-a" = "dummy-a"
    """
    parsed_toml = tomllib.loads(toml)
    manifest.write_text(toml)
    dummy_a = tmp_path / "bin" / exec_extension("dummy-a")

    # Test basic commands
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=env)
    assert dummy_a.is_file()

    # Add another expose
    dummy_in_disguise_str = exec_extension("dummy-in-disguise")
    dummy_in_disguise = tmp_path / "bin" / dummy_in_disguise_str
    parsed_toml["envs"]["test"]["exposed"][dummy_in_disguise_str] = "dummy-a"
    manifest.write_text(tomli_w.dumps(parsed_toml))
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=env)
    assert dummy_in_disguise.is_file()

    # Remove expose again
    del parsed_toml["envs"]["test"]["exposed"][dummy_in_disguise_str]
    manifest.write_text(tomli_w.dumps(parsed_toml))
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=env)
    assert not dummy_in_disguise.is_file()


def test_global_sync_manually_remove_binary(pixi: Path, tmp_path: Path, test_data: Path) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    dummy_channel = test_data.joinpath("dummy_channel_1/output").as_uri()
    toml = f"""
    [envs.test]
    channels = ["{dummy_channel}"]
    [envs.test.dependencies]
    dummy-a = "*"

    [envs.test.exposed]
    "dummy-a" = "dummy-a"
    """
    manifest.write_text(toml)
    dummy_a = tmp_path / "bin" / exec_extension("dummy-a")

    # Test basic commands
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=env)
    assert dummy_a.is_file()

    # Remove binary manually
    dummy_a.unlink()

    # Binary is added again
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=env)
    assert dummy_a.is_file()


def test_global_sync_migrate(pixi: Path, tmp_path: Path, test_data: Path) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    dummy_channel = test_data.joinpath("dummy_channel_1/output").as_uri()
    toml = f"""
    [envs.test]
    channels = ["{dummy_channel}"]
    [envs.test.dependencies]
    dummy-a = "*"
    dummy-b = "*"

    [envs.test.exposed]
    dummy-1 = "dummy-a"
    dummy-2 = "dummy-a"
    dummy-3 = "dummy-b"
    dummy-4 = "dummy-b"
    """
    manifest.write_text(toml)
    verify_cli_command([pixi, "global", "sync"], ExitCode.SUCCESS, env=env)

    # Test migration from existing environments
    original_manifest = tomllib.loads(manifest.read_text())
    manifest.unlink()
    verify_cli_command([pixi, "global", "sync", "--assume-yes"], ExitCode.SUCCESS, env=env)
    migrated_manifest = tomllib.loads(manifest.read_text())
    assert migrated_manifest == original_manifest


def test_global_expose_basic(pixi: Path, tmp_path: Path, test_data: Path) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    dummy_channel = test_data.joinpath("dummy_channel_1/output").as_uri()
    toml = f"""
    [envs.test]
    channels = ["{dummy_channel}"]
    [envs.test.dependencies]
    dummy-a = "*"
    """
    manifest.write_text(toml)
    dummy1 = tmp_path / "bin" / exec_extension("dummy1")
    dummy3 = tmp_path / "bin" / exec_extension("dummy3")

    # Add dummy1
    verify_cli_command(
        [pixi, "global", "expose", "add", "--environment=test", "dummy1=dummy-a"],
        ExitCode.SUCCESS,
        env=env,
    )
    assert dummy1.is_file()

    # Add dummy3
    verify_cli_command(
        [pixi, "global", "expose", "add", "--environment=test", "dummy3=dummy-a"],
        ExitCode.SUCCESS,
        env=env,
    )
    assert dummy3.is_file()

    # Remove dummy1
    verify_cli_command(
        [pixi, "global", "expose", "remove", "--environment=test", "dummy1"],
        ExitCode.SUCCESS,
        env=env,
    )
    assert not dummy1.is_file()

    # Attempt to remove python2
    verify_cli_command(
        [pixi, "global", "expose", "remove", "--environment=test", "dummy2"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains="The exposed name dummy2 doesn't exist",
    )


def test_global_expose_revert_working(pixi: Path, tmp_path: Path, test_data: Path) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    dummy_channel = test_data.joinpath("dummy_channel_1/output").as_uri()
    original_toml = f"""
    [envs.test]
    channels = ["{dummy_channel}"]
    [envs.test.dependencies]
    dummy-a = "*"
    """
    manifest.write_text(original_toml)

    # Attempt to add executable dummy-b that is not in our dependencies
    verify_cli_command(
        [pixi, "global", "expose", "add", "--environment=test", "dummy-b=dummy-b"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains="Could not find dummy-b in test",
    )

    # The TOML has been reverted to the original state
    assert original_toml == manifest.read_text()


def test_global_expose_revert_failure(pixi: Path, tmp_path: Path, test_data: Path) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    dummy_channel = test_data.joinpath("dummy_channel_1/output").as_uri()
    original_toml = f"""
    [envs.test]
    channels = ["{dummy_channel}"]
    [envs.test.dependencies]
    dummy-a = "*"
    [envs.test.exposed]
    dummy1 = "dummy-b"
    """
    manifest.write_text(original_toml)

    # Attempt to add executable dummy-b that isn't in our dependencies
    # It should fail since the original manifest contains "dummy-b",
    # which is not in our dependencies
    verify_cli_command(
        [pixi, "global", "expose", "add", "--environment=test", "dummy2=dummyb"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains="Could not add exposed mappings. Reverting also failed",
    )


def test_global_install_multiple_packages(pixi: Path, tmp_path: Path, test_data: Path) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    dummy_channel = test_data.joinpath("dummy_channel_1/output").as_uri()

    dummy_a = tmp_path / "bin" / exec_extension("dummy-a")
    dummy_aa = tmp_path / "bin" / exec_extension("dummy-aa")
    dummy_b = tmp_path / "bin" / exec_extension("dummy-b")
    dummy_c = tmp_path / "bin" / exec_extension("dummy-c")

    # Install dummy-a and dummy-b, even though dummy-c is a dependency of dummy-b, it should not be exposed
    # All of dummy-a's and dummy-b's executables should be exposed though: 'dummy-a', 'dummy-aa' and 'dummy-b'
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel,
            "dummy-a",
            "dummy-b",
        ],
        ExitCode.SUCCESS,
        env=env,
    )
    assert dummy_a.is_file()
    assert dummy_aa.is_file()
    assert dummy_b.is_file()
    assert not dummy_c.is_file()


def test_global_install_expose(pixi: Path, tmp_path: Path, test_data: Path) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    dummy_channel = test_data.joinpath("dummy_channel_1/output").as_uri()

    dummy_a = tmp_path / "bin" / exec_extension("dummy-a")
    dummy_aa = tmp_path / "bin" / exec_extension("dummy-aa")
    dummy_c = tmp_path / "bin" / exec_extension("dummy-c")

    # Install dummy-a, even though dummy-c is a dependency, it should not be exposed
    # All of dummy-a's executables should be exposed though: 'dummy-a' and 'dummy-aa'
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel,
            "dummy-a",
        ],
        ExitCode.SUCCESS,
        env=env,
    )
    assert dummy_a.is_file()
    assert dummy_aa.is_file()
    assert not dummy_c.is_file()

    # Install dummy-a, and expose dummy-c explicitly
    # Only dummy-c should now be exposed
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel,
            "--expose",
            "dummy-c=dummy-c",
            "dummy-a",
        ],
        ExitCode.SUCCESS,
        env=env,
    )
    assert not dummy_a.is_file()
    assert not dummy_aa.is_file()
    assert dummy_c.is_file()

    # Multiple mappings works as well
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel,
            "--expose",
            "dummy-a=dummy-a",
            "--expose",
            "dummy-aa=dummy-aa",
            "--expose",
            "dummy-c=dummy-c",
            "dummy-a",
        ],
        ExitCode.SUCCESS,
        env=env,
    )
    assert dummy_a.is_file()
    assert dummy_aa.is_file()
    assert dummy_c.is_file()

    # Expose doesn't work with multiple environments
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel,
            "--expose",
            "dummy-a=dummy-a",
            "dummy-a",
            "dummy-b",
        ],
        ExitCode.FAILURE,
        env=env,
        stderr_contains="Cannot add exposed mappings for more than one environment",
    )

    # But it does work with multiple packages and a single environment
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel,
            "--environment",
            "common-env",
            "--expose",
            "dummy-a=dummy-a",
            "dummy-a",
            "dummy-b",
        ],
        ExitCode.SUCCESS,
        env=env,
    )


def test_global_install_platform(pixi: Path, tmp_path: Path) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    # Exists on win-64
    verify_cli_command(
        [pixi, "global", "install", "--platform", "win-64", "binutils=2.40"],
        ExitCode.SUCCESS,
        env=env,
    )

    # Does not exist on osx-64
    verify_cli_command(
        [pixi, "global", "install", "--platform", "osx-64", "binutils=2.40"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains="No candidates were found",
    )


def test_global_install_channels(pixi: Path, tmp_path: Path, test_data: Path) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    dummy_channel_1 = test_data.joinpath("dummy_channel_1/output").as_uri()
    dummy_channel_2 = test_data.joinpath("dummy_channel_2/output").as_uri()

    dummy_b = tmp_path / "bin" / exec_extension("dummy-b")
    dummy_x = tmp_path / "bin" / exec_extension("dummy-x")

    # Install dummy-b from dummy-channel-1
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "dummy-b",
        ],
        ExitCode.SUCCESS,
        env=env,
    )
    assert dummy_b.is_file()

    # Install dummy-x from dummy-channel-2
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_2,
            "dummy-x",
        ],
        ExitCode.SUCCESS,
        env=env,
    )
    assert dummy_x.is_file()

    # Install dummy-b and dummy-x from dummy-channel-1 and dummy-channel-2
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "--channel",
            dummy_channel_2,
            "dummy-b",
            "dummy-x",
        ],
        ExitCode.SUCCESS,
        env=env,
    )
    assert dummy_b.is_file()
    assert dummy_x.is_file()
