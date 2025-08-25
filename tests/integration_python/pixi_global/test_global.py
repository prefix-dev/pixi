import platform
import shutil
import tomllib
from pathlib import Path

import pytest
import tomli_w

from ..common import ExitCode, bat_extension, exec_extension, verify_cli_command

MANIFEST_VERSION = 1


@pytest.mark.slow
def test_sync_injected_python_lib_is_found(pixi: Path, tmp_path: Path) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    toml = """
    [envs.test]
    channels = ["https://prefix.dev/conda-forge"]
    dependencies = { python = "==3.13.0" }
    exposed = { "python-injected" = "python" }
    """
    parsed_toml = tomllib.loads(toml)
    manifest.write_text(toml)
    python_injected = tmp_path / "bin" / exec_extension("python-injected")

    # Test basic commands
    verify_cli_command([pixi, "global", "sync"], env=env)
    verify_cli_command([python_injected, "--version"], env=env, stdout_contains="3.13.0")
    verify_cli_command(
        [python_injected, "-c", "import narwhals; print(narwhals.__version__)"],
        ExitCode.FAILURE,
        env=env,
    )

    # Add narwhals
    parsed_toml["envs"]["test"]["dependencies"]["narwhals"] = "1.29.0"
    manifest.write_text(tomli_w.dumps(parsed_toml))
    verify_cli_command([pixi, "global", "sync"], env=env)
    verify_cli_command(
        [python_injected, "-c", "import narwhals; print(narwhals.__version__)"],
        env=env,
        stdout_contains="1.29.0",
    )

    # Remove narwhals again
    del parsed_toml["envs"]["test"]["dependencies"]["narwhals"]
    manifest.write_text(tomli_w.dumps(parsed_toml))
    verify_cli_command([pixi, "global", "sync"], env=env)
    verify_cli_command(
        [python_injected, "-c", "import narwhals; print(narwhals.__version__)"],
        ExitCode.FAILURE,
        env=env,
    )

    # Remove python
    del parsed_toml["envs"]["test"]["dependencies"]["python"]
    manifest.write_text(tomli_w.dumps(parsed_toml))
    verify_cli_command(
        [pixi, "global", "sync"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains=[
            "Couldn't find executable",
            "Failed to add executables for environment",
        ],
    )


@pytest.mark.slow
def test_sync_platform(pixi: Path, tmp_path: Path) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    toml = """
    [envs.test]
    channels = ["https://prefix.dev/conda-forge"]
    platform = "win-64"
    dependencies = { binutils = "2.40.*" }\
    """
    parsed_toml = tomllib.loads(toml)
    manifest.write_text(toml)

    # Exists on win-64
    verify_cli_command([pixi, "global", "sync"], env=env)

    # Doesn't exist on osx-64
    parsed_toml["envs"]["test"]["platform"] = "osx-64"
    manifest.write_text(tomli_w.dumps(parsed_toml))
    verify_cli_command(
        [pixi, "global", "sync"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains="No candidates were found",
    )


def test_sync_change_expose(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    toml = f"""
    [envs.test]
    channels = ["{dummy_channel_1}"]
    [envs.test.dependencies]
    dummy-a = "*"

    [envs.test.exposed]
    "dummy-a" = "dummy-a"
    """
    parsed_toml = tomllib.loads(toml)
    manifest.write_text(toml)
    dummy_a = tmp_path / "bin" / exec_extension("dummy-a")

    # Test basic commands
    verify_cli_command([pixi, "global", "sync"], env=env)
    assert dummy_a.is_file()

    # Add another expose
    dummy_in_disguise = tmp_path / "bin" / exec_extension("dummy-in-disguise")
    parsed_toml["envs"]["test"]["exposed"]["dummy-in-disguise"] = "dummy-a"
    manifest.write_text(tomli_w.dumps(parsed_toml))
    verify_cli_command([pixi, "global", "sync"], env=env)
    assert dummy_in_disguise.is_file()

    # Remove expose again
    del parsed_toml["envs"]["test"]["exposed"]["dummy-in-disguise"]
    manifest.write_text(tomli_w.dumps(parsed_toml))
    verify_cli_command([pixi, "global", "sync"], env=env)
    assert not dummy_in_disguise.is_file()


def test_sync_prune(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    toml = f"""
    [envs.test]
    channels = ["{dummy_channel_1}"]
    dependencies = {{ dummy-a = "*" }}
    exposed = {{ dummy-a = "dummy-a"}}
    """
    parsed_toml = tomllib.loads(toml)
    manifest.write_text(toml)
    dummy_a = tmp_path / "bin" / exec_extension("dummy-a")

    # Test basic commands
    verify_cli_command([pixi, "global", "sync"], env=env)
    assert dummy_a.is_file()

    # Remove environment
    del parsed_toml["envs"]["test"]
    manifest.write_text(tomli_w.dumps(parsed_toml))
    verify_cli_command(
        [pixi, "global", "sync"], env=env, stderr_contains="Removed environment test"
    )
    assert not dummy_a.is_file()


def test_sync_manually_remove_binary(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    toml = f"""
    [envs.test]
    channels = ["{dummy_channel_1}"]
    [envs.test.dependencies]
    dummy-a = "*"

    [envs.test.exposed]
    "dummy-a" = "dummy-a"
    """
    manifest.write_text(toml)
    dummy_a = tmp_path / "bin" / exec_extension("dummy-a")

    # Test basic commands
    verify_cli_command([pixi, "global", "sync"], env=env)
    assert dummy_a.is_file()

    # Remove binary manually
    dummy_a.unlink()

    # Binary is added again
    verify_cli_command([pixi, "global", "sync"], env=env)
    assert dummy_a.is_file()


@pytest.mark.slow
def test_sync_migrate(
    pixi: Path, tmp_path: Path, dummy_channel_1: str, dummy_channel_2: str
) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    toml = f"""\
version = {MANIFEST_VERSION}
# Test with special channel
[envs.test]
channels = ["{dummy_channel_1}"]
dependencies = {{ dummy-a = "*", dummy-b = "*" }}
exposed = {{ dummy-1 = "dummy-a", dummy-2 = "dummy-a", dummy-3 = "dummy-b", dummy-4 = "dummy-b" }}

# Test with multiple channels
[envs.test1]
channels = ["{dummy_channel_1}", "{dummy_channel_2}"]
dependencies = {{ dummy-d = "*" }}
exposed = {{ dummy-d = "dummy-d" }}

# Test with conda-forge channel
[envs.test2]
channels = ["https://prefix.dev/conda-forge"]
# Small package with binary for testing purposes
dependencies = {{ xz-tools = "*" }}
exposed = {{ xz = "xz" }}
"""
    manifest.write_text(toml)
    verify_cli_command([pixi, "global", "sync"], env=env)

    # Test migration from existing environments
    original_manifest = manifest.read_text()
    manifest.unlink()
    manifests.rmdir()
    verify_cli_command([pixi, "global", "sync"], env=env)
    migrated_manifest = manifest.read_text()
    assert tomllib.loads(migrated_manifest) == tomllib.loads(original_manifest)


def test_sync_duplicated_expose_error(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    toml = f"""
version = {MANIFEST_VERSION}

[envs.one]
channels = ["{dummy_channel_1}"]
dependencies = {{ dummy-a = "*" }}
exposed = {{ dummy-1 = "dummy-a" }}

[envs.two]
channels = ["{dummy_channel_1}"]
dependencies = {{ dummy-b = "*" }}
exposed = {{ dummy-1 = "dummy-b" }}
    """
    manifest.write_text(toml)
    verify_cli_command(
        [pixi, "global", "sync"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains="Duplicated exposed names found: dummy-1",
    )


def test_sync_failures_are_collected(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    toml = f"""
version = {MANIFEST_VERSION}

[envs.one]
channels = ["{dummy_channel_1}"]
dependencies = {{ invalid-package = "*" }}
exposed = {{ }}

[envs.two]
channels = ["{dummy_channel_1}"]
dependencies = {{ another-invalid-package = "*" }}
exposed = {{ }}

[envs.three]
channels = ["{dummy_channel_1}"]
dependencies = {{ dummy-a = "*" }}
exposed = {{ dummy-a = "dummy-a" }}
    """
    manifest.write_text(toml)
    dummy_a = tmp_path / "bin" / exec_extension("dummy-a")

    verify_cli_command(
        [pixi, "global", "sync"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains=["invalid-package", "another-invalid-package"],
    )
    assert dummy_a.is_file()


def test_expose_basic(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    toml = f"""
    [envs.test]
    channels = ["{dummy_channel_1}"]
    dependencies = {{ dummy-a = "*" }}
    """
    manifest.write_text(toml)
    dummy_a = tmp_path / "bin" / exec_extension("dummy-a")
    dummy1 = tmp_path / "bin" / exec_extension("dummy1")
    dummy3 = tmp_path / "bin" / exec_extension("dummy3")
    nested_dummy = tmp_path / "bin" / exec_extension("dummy")

    # Add dummy-a with simple syntax
    verify_cli_command([pixi, "global", "expose", "add", "--environment=test", "dummy-a"], env=env)
    assert dummy_a.is_file()

    # Add dummy1 and dummy3 and nested/dummy
    verify_cli_command(
        [
            pixi,
            "global",
            "expose",
            "add",
            "--environment=test",
            "dummy1=dummy-a",
            "dummy3=dummy-a",
            # Test nested expose without naming it through cli, as it should become the name of the executable
            "nested/dummy",
        ],
        env=env,
    )
    assert dummy1.is_file()
    assert dummy3.is_file()
    assert nested_dummy.is_file()

    # Remove dummy-a
    verify_cli_command(
        [pixi, "global", "expose", "remove", "dummy-a"],
        env=env,
    )
    assert not dummy_a.is_file()

    # Remove dummy1 and dummy3 and attempt to remove dummy2
    verify_cli_command(
        [pixi, "global", "expose", "remove", "dummy1", "dummy3", "dummy2"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains="Exposed name dummy2 not found in any environment",
    )
    assert not dummy1.is_file()
    assert not dummy3.is_file()


def test_expose_revert_working(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    original_toml = f"""
    [envs.test]
    channels = ["{dummy_channel_1}"]
    dependencies = {{ dummy-a = "*" }}
    """
    manifest.write_text(original_toml)

    # Attempt to add executable dummy-b that is not in our dependencies
    verify_cli_command(
        [pixi, "global", "expose", "add", "--environment=test", "dummy-b=dummy-b"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains=["Couldn't find executable dummy-b in", "test", "executables"],
    )

    # The TOML has been reverted to the original state
    assert manifest.read_text() == original_toml


def test_expose_preserves_table_format(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    original_toml = f"""
version = {MANIFEST_VERSION}

[envs.test]
channels = ["{dummy_channel_1}"]
[envs.test.dependencies]
dummy-a = "*"
[envs.test.exposed]
dummy-a = "dummy-a"
"""
    manifest.write_text(original_toml)

    verify_cli_command(
        [pixi, "global", "expose", "add", "--environment=test", "dummy-aa=dummy-a"],
        env=env,
    )
    # The tables in the manifest have been preserved
    assert manifest.read_text() == original_toml + 'dummy-aa = "dummy-a"\n'


def test_expose_duplicated_expose_allow_for_same_env(
    pixi: Path, tmp_path: Path, dummy_channel_1: str
) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    toml = f"""
version = {MANIFEST_VERSION}

[envs.one]
channels = ["{dummy_channel_1}"]
dependencies = {{ dummy-a = "*", dummy-b = "*" }}
exposed = {{ dummy-1 = "dummy-a" }}

[envs.two]
channels = ["{dummy_channel_1}"]
dependencies = {{ dummy-a = "*", dummy-b = "*" }}
exposed = {{ dummy-2 = "dummy-a" }}
"""
    manifest.write_text(toml)

    verify_cli_command(
        [pixi, "global", "sync"],
        env=env,
    )

    # This will not work sinced there would be two times `dummy-2` after this command
    verify_cli_command(
        [pixi, "global", "expose", "add", "--environment", "one", "dummy-2=dummy-b"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains="Exposed name dummy-2 already exists",
    )

    # This should work, since it just overwrites the existing `dummy-2` mapping
    verify_cli_command(
        [pixi, "global", "expose", "add", "--environment", "two", "dummy-2=dummy-b"],
        env=env,
    )
    parsed_toml = tomllib.loads(manifest.read_text())
    assert parsed_toml["envs"]["two"]["exposed"]["dummy-2"] == "dummy-b"


def test_install_duplicated_expose_allow_for_same_env(
    pixi: Path, tmp_path: Path, dummy_channel_1: str
) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")

    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "dummy-a",
            "--expose",
            "dummy=dummy-a",
            "--channel",
            dummy_channel_1,
        ],
        env=env,
    )

    # This will not work sinced there would be two times `dummy` after this command
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "dummy-b",
            "--expose",
            "dummy=dummy-b",
            "--channel",
            dummy_channel_1,
        ],
        ExitCode.FAILURE,
        env=env,
        stderr_contains="Exposed name dummy already exists",
    )

    # This should work, since it just overwrites the existing properties of environment `dummy-a`
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "dummy-a",
            "--expose",
            "dummy=dummy-aa",
            "--channel",
            dummy_channel_1,
            "-vvvv",
        ],
        env=env,
    )
    parsed_toml = tomllib.loads(manifest.read_text())
    assert parsed_toml["envs"]["dummy-a"]["exposed"]["dummy"] == "dummy-aa"


def test_install_adapts_manifest(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    original_toml = f"""
    [envs.test]
    channels = ["{dummy_channel_1}"]
    dependencies= {{ dummy-b = "*" }}
    exposed = {{ dummy-b = "dummy-b" }}
    """
    manifest.write_text(original_toml)

    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "dummy-a",
        ],
        env=env,
    )

    assert f"version = {MANIFEST_VERSION}" in manifest.read_text()


def test_existing_manifest_gets_version(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifest = manifests.joinpath("pixi-global.toml")

    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "dummy-a",
        ],
        env=env,
    )

    expected_manifest = f"""\
version = {MANIFEST_VERSION}

[envs.dummy-a]
channels = ["{dummy_channel_1}"]
dependencies = {{ dummy-a = "*" }}
exposed = {{ dummy-a = "dummy-a", dummy-aa = "dummy-aa" }}
"""
    actual_manifest = manifest.read_text()

    # Ensure that the manifest is correctly adapted
    assert actual_manifest == expected_manifest


def test_install_with_basic(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifest = manifests.joinpath("pixi-global.toml")

    dummy_a = tmp_path / "bin" / exec_extension("dummy-a")
    dummy_aa = tmp_path / "bin" / exec_extension("dummy-aa")
    dummy_b = tmp_path / "bin" / exec_extension("dummy-b")
    dummy_c = tmp_path / "bin" / exec_extension("dummy-c")

    # Should fail, since two environments are created
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "dummy-a",
            "dummy-b",
            "--with",
            "dummy-c",
        ],
        ExitCode.FAILURE,
        env=env,
        stderr_contains="Can't add packages with `--with` for more than one environment",
    )

    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "dummy-a",
            "--with",
            "dummy-b",
            "--with",
            "dummy-c",
        ],
        env=env,
    )

    expected_manifest = f"""\
version = {MANIFEST_VERSION}

[envs.dummy-a]
channels = ["{dummy_channel_1}"]
dependencies = {{ dummy-a = "*", dummy-b = "*", dummy-c = "*" }}
exposed = {{ dummy-a = "dummy-a", dummy-aa = "dummy-aa" }}
"""
    actual_manifest = manifest.read_text()

    # Ensure that the manifest is correctly adapted
    assert actual_manifest == expected_manifest

    assert dummy_a.is_file()
    assert dummy_aa.is_file()
    assert not dummy_b.is_file()
    assert not dummy_c.is_file()


def test_install_with_environment_no_expose(
    pixi: Path, tmp_path: Path, dummy_channel_1: str
) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifest = manifests.joinpath("pixi-global.toml")

    dummy_a = tmp_path / "bin" / exec_extension("dummy-a")
    dummy_aa = tmp_path / "bin" / exec_extension("dummy-aa")
    dummy_b = tmp_path / "bin" / exec_extension("dummy-b")

    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "--environment",
            "dummy",
            "dummy-a",
            "--with",
            "dummy-b",
        ],
        env=env,
    )

    expected_manifest = f"""\
version = {MANIFEST_VERSION}

[envs.dummy]
channels = ["{dummy_channel_1}"]
dependencies = {{ dummy-a = "*", dummy-b = "*" }}
exposed = {{ dummy-a = "dummy-a", dummy-aa = "dummy-aa" }}
"""
    actual_manifest = manifest.read_text()

    # Ensure that the manifest is correctly adapted
    assert actual_manifest == expected_manifest

    assert dummy_a.is_file()
    assert dummy_aa.is_file()
    assert not dummy_b.is_file()


def test_install_with_environment_and_expose(
    pixi: Path, tmp_path: Path, dummy_channel_1: str
) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifest = manifests.joinpath("pixi-global.toml")

    dummy_a = tmp_path / "bin" / exec_extension("dummy-a")
    dummy_aa = tmp_path / "bin" / exec_extension("dummy-aa")
    dummy_b = tmp_path / "bin" / exec_extension("dummy-b")

    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "--environment",
            "dummy",
            "--expose=dummy-b",
            "dummy-a",
            "--with",
            "dummy-b",
        ],
        env=env,
    )

    expected_manifest = f"""\
version = {MANIFEST_VERSION}

[envs.dummy]
channels = ["{dummy_channel_1}"]
dependencies = {{ dummy-a = "*", dummy-b = "*" }}
exposed = {{ dummy-b = "dummy-b" }}
"""
    actual_manifest = manifest.read_text()

    # Ensure that the manifest is correctly adapted
    assert actual_manifest == expected_manifest

    assert not dummy_a.is_file()
    assert not dummy_aa.is_file()
    assert dummy_b.is_file()


def test_install_twice(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    dummy_b = tmp_path / "bin" / exec_extension("dummy-b")

    # Install dummy-b
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "dummy-b",
        ],
        env=env,
        stdout_contains="dummy-b: 0.1.0 (installed)",
    )
    assert dummy_b.is_file()

    # Install dummy-b again, there should be nothing to do
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "dummy-b",
        ],
        env=env,
        stdout_contains="dummy-b: 0.1.0 (already installed)",
    )
    assert dummy_b.is_file()


def test_install_twice_with_same_env_name_as_expose(
    pixi: Path, tmp_path: Path, dummy_channel_1: str
) -> None:
    # This test is to ensure that when the environment name is the same as the expose name, exposes are printed correctly
    # and we also ensure that when custom name for environment is used,
    # we output state for it
    env = {"PIXI_HOME": str(tmp_path)}

    dummy_b = tmp_path / "bin" / exec_extension("customdummyb")

    # Install dummy-b
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "dummy-b",
            "--environment",
            "customdummyb",
            "--expose",
            "customdummyb=dummy-b",
        ],
        env=env,
        stdout_contains=["customdummyb (installed)", "exposes: customdummyb -> dummy-b"],
    )
    assert dummy_b.is_file()

    # Install dummy-b again, there should be nothing to do
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "dummy-b",
            "--environment",
            "customdummyb",
            "--expose",
            "customdummyb=dummy-b",
        ],
        env=env,
        stdout_contains=["customdummyb (already installed)", "exposes: customdummyb -> dummy-b"],
    )
    assert dummy_b.is_file()


def test_install_force_reinstall_removes_original_environment(
    pixi: Path, tmp_path: Path, dummy_channel_1: str
) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    dummy_b = tmp_path / "bin" / exec_extension("dummy-b")
    dummy_c = tmp_path / "bin" / exec_extension("dummy-c")

    env_name = "test_env"

    # Install dummy-b
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "--environment",
            env_name,
            "dummy-b",
        ],
        env=env,
    )
    assert dummy_b.is_file()
    assert not dummy_c.is_file()

    # Install dummy-c, it should be added to the environment
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "--environment",
            env_name,
            "dummy-c",
        ],
        env=env,
    )
    assert dummy_b.is_file()
    assert dummy_c.is_file()

    # Install dummy-c with `--force-reinstall
    # It should create a fresh environment
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "--environment",
            env_name,
            "dummy-c",
            "--force-reinstall",
        ],
        env=env,
    )
    assert not dummy_b.is_file()
    assert dummy_c.is_file()


def test_install_with_different_channel_and_force_reinstall(
    pixi: Path, tmp_path: Path, dummy_channel_1: str, dummy_channel_2: str
) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    dummy_b = tmp_path / "bin" / exec_extension("dummy-b")

    # Install dummy-b
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "dummy-b",
        ],
        env=env,
        stdout_contains="dummy-b: 0.1.0 (installed)",
    )
    assert dummy_b.is_file()

    # Modify dummy-b channel and try to install it again
    # Even though we changed the channels, it will claim the environment is up-to-date

    manifests = tmp_path / "manifests" / "pixi-global.toml"
    parsed_toml = tomllib.loads(manifests.read_text())

    parsed_toml["envs"]["dummy-b"]["channels"] = [dummy_channel_2]

    manifests.write_text(tomli_w.dumps(parsed_toml))

    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_2,
            "dummy-b",
        ],
        env=env,
        stdout_contains="dummy-b: 0.1.0 (already installed)",
    )

    # Install dummy-b again, but with force-reinstall
    # It should install it again
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--force-reinstall",
            "--channel",
            dummy_channel_2,
            "dummy-b",
        ],
        env=env,
        stdout_contains="dummy-b: 0.1.0 (installed)",
    )


def test_install_underscore(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    dummy_e = tmp_path / "bin" / exec_extension("dummy_e")

    # Install package `dummy_e`
    # It should be installed in environment `dummy-e`
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "dummy_e",
        ],
        env=env,
    )
    assert dummy_e.is_file()

    # Uninstall `dummy_e`
    # The `_` will again automatically be converted into an `-`
    verify_cli_command(
        [
            pixi,
            "global",
            "uninstall",
            "dummy_e",
        ],
        env=env,
    )
    assert not dummy_e.is_file()


def test_install_multiple_packages(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

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
            dummy_channel_1,
            "dummy-a",
            "dummy-b",
        ],
        env=env,
    )
    assert dummy_a.is_file()
    assert dummy_aa.is_file()
    assert dummy_b.is_file()
    assert not dummy_c.is_file()


def test_install_expose_single_package(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

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
            dummy_channel_1,
            "dummy-a",
        ],
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
            dummy_channel_1,
            "--expose",
            "dummy-c",
            "dummy-a",
        ],
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
            dummy_channel_1,
            "--expose",
            "dummy-a",
            "--expose",
            "dummy-aa",
            "--expose",
            "dummy-c",
            "dummy-a",
        ],
        env=env,
    )
    assert dummy_a.is_file()
    assert dummy_aa.is_file()
    assert dummy_c.is_file()


def test_install_expose_multiple_packages(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    dummy_a = tmp_path / "bin" / exec_extension("dummy-a")
    dummy_b = tmp_path / "bin" / exec_extension("dummy-b")

    # Expose doesn't work with multiple environments
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "--expose",
            "dummy-a",
            "dummy-a",
            "dummy-b",
        ],
        ExitCode.FAILURE,
        env=env,
        stderr_contains="Can't add exposed mappings with `--exposed` for more than one environment",
    )

    # But it does work with multiple packages and a single environment
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "--environment",
            "common-env",
            "--expose",
            "dummy-a",
            "dummy-a",
            "dummy-b",
        ],
        env=env,
    )

    assert dummy_a.is_file()
    assert not dummy_b.is_file()


def test_install_only_reverts_failing(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    dummy_a = tmp_path / "bin" / exec_extension("dummy-a")
    dummy_b = tmp_path / "bin" / exec_extension("dummy-b")
    dummy_x = tmp_path / "bin" / exec_extension("dummy-x")

    # dummy-x is not part of dummy_channel_1
    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_1, "dummy-a", "dummy-b", "dummy-x"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains="No candidates were found for dummy-x",
    )

    # dummy-a, dummy-b should be installed, but not dummy-x
    assert dummy_a.is_file()
    assert dummy_b.is_file()
    assert not dummy_x.is_file()


@pytest.mark.slow
def test_install_platform(pixi: Path, tmp_path: Path) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    # Exists on win-64
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--platform",
            "win-64",
            "binutils=2.40",
            "--channel",
            "https://prefix.dev/conda-forge",
        ],
        env=env,
    )

    # Doesn't exist on osx-64
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--platform",
            "osx-64",
            "binutils=2.40",
            "--channel",
            "https://prefix.dev/conda-forge",
        ],
        ExitCode.FAILURE,
        env=env,
        stderr_contains="No candidates were found",
    )


def test_install_channels(
    pixi: Path, tmp_path: Path, dummy_channel_1: str, dummy_channel_2: str
) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
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
        env=env,
    )
    assert dummy_b.is_file()
    assert dummy_x.is_file()


def test_install_multi_env_install(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    # Install dummy-a and dummy-b from dummy-channel-1 this will fail if both environment contains the same package as spec.
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "dummy-a",
            "dummy-b",
        ],
        env=env,
    )


@pytest.mark.skipif(platform.system() == "Windows", reason="Not reliable on Windows")
def test_pixi_install_cleanup(pixi: Path, tmp_path: Path, multiple_versions_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    package0_1_0 = tmp_path / "bin" / exec_extension("package0.1.0")
    package0_2_0 = tmp_path / "bin" / exec_extension("package0.2.0")

    verify_cli_command(
        [pixi, "global", "install", "--channel", multiple_versions_channel_1, "package==0.1.0"],
        env=env,
    )
    assert package0_1_0.is_file()
    assert not package0_2_0.is_file()

    # Install the same package but with a different version
    # The old version should be removed and the new version should be installed without error.
    verify_cli_command(
        [pixi, "global", "install", "--channel", multiple_versions_channel_1, "package==0.2.0"],
        env=env,
    )

    assert not package0_1_0.is_file()
    assert package0_2_0.is_file()


def test_list(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()

    # Verify empty list
    verify_cli_command(
        [pixi, "global", "list"],
        env=env,
        stdout_contains="No global environments found.",
    )

    # Install dummy-b from dummy-channel-1
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "dummy-b==0.1.0",
            "dummy-a==0.1.0",
        ],
        env=env,
    )

    # Verify list with dummy-b
    verify_cli_command(
        [pixi, "global", "list"],
        env=env,
        stdout_contains=["dummy-b: 0.1.0", "dummy-a: 0.1.0", "dummy-a", "dummy-aa"],
    )


def test_list_env_no_dependencies(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    toml = f"""
    [envs.test]
    channels = ["{dummy_channel_1}"]
    dependencies = {{}}
    """
    manifest.write_text(toml)

    # Verify empty list
    verify_cli_command(
        [pixi, "global", "list"],
        env=env,
        stderr_contains="Environment test doesn't contain dependencies",
    )


def test_list_with_filter(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()

    # Install dummy-a and dummy-b from dummy-channel-1
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "dummy-b==0.1.0",
            "dummy-a==0.1.0",
        ],
        env=env,
    )

    # Verify list with dummy-a
    verify_cli_command(
        [pixi, "global", "list", "dummy-a"],
        env=env,
        stdout_contains=["dummy-a: 0.1.0", "dummy-a", "dummy-aa"],
        stdout_excludes=["dummy-b"],
    )

    # Verify list filter for environment dummy-a.
    # It should not contains dummy-b, but should contain dummy-a
    verify_cli_command(
        [pixi, "global", "list", "--environment", "dummy-a", "dummy"],
        env=env,
        stdout_contains=["The dummy-a environment", "dummy-a", "0.1.0"],
        stdout_excludes=["dummy-b"],
    )


# Test that we correctly uninstall the required packages
# - Checking that the binaries are removed
# - Checking that the non-requested to remove binaries are still there
def test_uninstall(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    original_toml = f"""
version = {MANIFEST_VERSION}
[envs.dummy-a]
channels = ["{dummy_channel_1}"]
dependencies = {{ dummy-a = "*" }}
exposed = {{ dummy-a = "dummy-a", dummy-aa = "dummy-aa" }}

[envs.dummy-b]
channels = ["{dummy_channel_1}"]
dependencies = {{ dummy-b = "*" }}
exposed = {{ dummy-b = "dummy-b" }}

[envs.dummy-c]
channels = ["{dummy_channel_1}"]
dependencies = {{ dummy-c = "*" }}
exposed = {{ dummy-c = "dummy-c" }}
"""
    manifest.write_text(original_toml)

    verify_cli_command(
        [
            pixi,
            "global",
            "sync",
        ],
        env=env,
    )
    dummy_a = tmp_path / "bin" / exec_extension("dummy-a")
    dummy_aa = tmp_path / "bin" / exec_extension("dummy-aa")
    dummy_b = tmp_path / "bin" / exec_extension("dummy-b")
    dummy_c = tmp_path / "bin" / exec_extension("dummy-c")
    assert dummy_a.is_file()
    assert dummy_aa.is_file()
    assert dummy_b.is_file()
    assert dummy_c.is_file()

    # Uninstall dummy-a
    verify_cli_command(
        [pixi, "global", "uninstall", "dummy-a"],
        env=env,
        stderr_contains="Removed environment dummy-a",
    )
    assert not dummy_a.is_file()
    assert not dummy_aa.is_file()
    assert dummy_b.is_file()
    assert dummy_c.is_file()
    # Verify only the dummy-a environment is removed
    assert tmp_path.joinpath("envs", "dummy-b").is_dir()
    assert tmp_path.joinpath("envs", "dummy-c").is_dir()
    assert not tmp_path.joinpath("envs", "dummy-a").is_dir()

    # Remove dummy-b manually from manifest
    modified_toml = f"""
version = {MANIFEST_VERSION}

[envs.dummy-c]
channels = ["{dummy_channel_1}"]
dependencies = {{ dummy-c = "*" }}
exposed = {{ dummy-c = "dummy-c" }}
"""
    manifest.write_text(modified_toml)

    # Uninstall dummy-c
    verify_cli_command(
        [pixi, "global", "uninstall", "dummy-c"],
        env=env,
    )
    assert not dummy_a.is_file()
    assert not dummy_aa.is_file()
    assert not dummy_c.is_file()
    # Verify only the dummy-c environment is removed, dummy-b is still there as no sync is run.
    assert dummy_b.is_file()

    # Verify empty list
    verify_cli_command(
        [pixi, "global", "list"],
        env=env,
        stdout_contains="No global environments found.",
    )

    # Uninstall non-existing package
    verify_cli_command(
        [pixi, "global", "uninstall", "dummy-a"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains="Couldn't remove dummy-a",
    )

    # Uninstall multiple packages
    manifest.write_text(original_toml)

    verify_cli_command(
        [
            pixi,
            "global",
            "sync",
        ],
        env=env,
    )
    assert dummy_a.is_file()
    assert dummy_aa.is_file()
    assert dummy_b.is_file()
    assert dummy_c.is_file()

    verify_cli_command(
        [pixi, "global", "uninstall", "dummy-a", "dummy-b"],
        env=env,
    )
    assert not dummy_a.is_file()
    assert not dummy_aa.is_file()
    assert not dummy_b.is_file()
    assert dummy_c.is_file()


def test_uninstall_only_reverts_failing(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    dummy_a = tmp_path / "bin" / exec_extension("dummy-a")
    dummy_b = tmp_path / "bin" / exec_extension("dummy-b")

    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_1, "dummy-a", "dummy-b"],
        env=env,
    )

    # We did not install dummy-c
    verify_cli_command(
        [pixi, "global", "uninstall", "dummy-a", "dummy-c"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains="Environment dummy-c doesn't exist",
    )

    # dummy-a has been removed but dummy-b is still there
    assert not dummy_a.is_file()
    assert not tmp_path.joinpath("envs", "dummy-a").is_dir()
    assert dummy_b.is_file()
    assert tmp_path.joinpath("envs", "dummy-b").is_dir()


def test_global_update_single_package(
    pixi: Path, tmp_path: Path, multiple_versions_channel_1: str
) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    # Test update with no environments
    verify_cli_command(
        [pixi, "global", "update"],
        env=env,
    )

    # Test update of a single package
    verify_cli_command(
        [pixi, "global", "install", "--channel", multiple_versions_channel_1, "package==0.1.0"],
        env=env,
    )
    # Replace the version with a "*"
    manifest = tmp_path.joinpath("manifests", "pixi-global.toml")
    manifest.write_text(manifest.read_text().replace("==0.1.0", "*"))
    verify_cli_command(
        [pixi, "global", "update", "package"],
        env=env,
        stderr_contains=["Updated", "package", "0.1.0", "0.2.0"],
    )
    package = tmp_path / "bin" / exec_extension("package")
    package0_1_0 = tmp_path / "bin" / exec_extension("package0.1.0")
    package0_2_0 = tmp_path / "bin" / exec_extension("package0.2.0")

    # After update be left with only the binary that was in both versions.
    assert package.is_file()
    assert not package0_1_0.is_file()
    # pixi global update should add new exposed mappings, as all of them were exposed before
    assert package0_2_0.is_file()


def test_global_update_single_package_with_transient_dependency(
    pixi: Path,
    tmp_path: Path,
    non_self_expose_channel_1: str,
    non_self_expose_channel_2: str,
) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    # Test update of a single package
    verify_cli_command(
        [pixi, "global", "install", "--channel", non_self_expose_channel_1, "jupyter==0.1.0"],
        env=env,
    )
    # Replace the version with a "*"
    manifest = tmp_path.joinpath("manifests", "pixi-global.toml")
    manifest.write_text(manifest.read_text().replace("==0.1.0", "*"))
    manifest_dict = tomllib.loads(manifest.read_text())
    manifest_dict["envs"]["jupyter"]["channels"] = [non_self_expose_channel_2]
    manifest.write_text(tomli_w.dumps(manifest_dict))
    # We updated only the transient dependency
    verify_cli_command(
        [pixi, "global", "update", "jupyter"],
        env=env,
        stderr_contains="Updated environment jupyter.",
    )


def test_global_update_doesnt_remove_exposed_key_of_transient_dependencies(
    pixi: Path, tmp_path: Path, multiple_versions_channel_1: Path
) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    package3 = tmp_path / "bin" / exec_extension("package3")
    package4 = tmp_path / "bin" / exec_extension("package4")

    # Install package4 which depends on package3
    verify_cli_command(
        [pixi, "global", "install", "--channel", multiple_versions_channel_1, "package4==0.1.0"],
        env=env,
    )
    assert not package3.is_file()
    assert package4.is_file()

    # Expose package3
    verify_cli_command(
        [pixi, "global", "expose", "add", "--environment", "package4", "package3"],
        env=env,
    )

    assert package3.is_file()
    assert package4.is_file()

    # Replace the version with a "*"
    manifest = tmp_path.joinpath("manifests", "pixi-global.toml")
    manifest.write_text(manifest.read_text().replace("==0.1.0", "*"))

    # Update environment
    verify_cli_command(
        [pixi, "global", "update", "package4"],
        env=env,
        stderr_contains=["Updated", "package4", "0.1.0", "0.2.0"],
    )

    # package3 should still be exposed
    assert package3.is_file()
    assert package4.is_file()


def test_global_update_all_packages(
    pixi: Path, tmp_path: Path, multiple_versions_channel_1: str
) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            multiple_versions_channel_1,
            "package2==0.1.0",
            "package==0.1.0",
        ],
        env=env,
    )

    package = tmp_path / "bin" / exec_extension("package")
    package0_1_0 = tmp_path / "bin" / exec_extension("package0.1.0")
    package0_2_0 = tmp_path / "bin" / exec_extension("package0.2.0")
    package2 = tmp_path / "bin" / exec_extension("package2")
    assert package2.is_file()
    assert package.is_file()
    assert package0_1_0.is_file()
    assert not package0_2_0.is_file()

    # Replace the version with a "*"
    manifest = tmp_path.joinpath("manifests", "pixi-global.toml")
    manifest.write_text(manifest.read_text().replace("==0.1.0", "*"))

    verify_cli_command(
        [pixi, "global", "update"],
        env=env,
    )
    assert package2.is_file()
    assert package.is_file()
    assert not package0_1_0.is_file()
    # After update be left we auto expose new binary, as all of them were exposed before
    assert package0_2_0.is_file()

    # Check the manifest for removed binaries
    manifest_content = manifest.read_text()
    assert "package0.1.0" not in manifest_content
    assert "package0.2.0" in manifest_content
    assert "package2" in manifest_content
    assert "package" in manifest_content

    # Check content of package2 file to be updated
    bin_file_package2 = tmp_path / "envs" / "package2" / "bin" / bat_extension("package2")
    assert "0.2.0" in bin_file_package2.read_text()


def test_global_update_multiple_packages_in_one_env(
    pixi: Path, tmp_path: Path, multiple_versions_channel_1: str
) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            multiple_versions_channel_1,
            "--environment",
            "my-packages",
            "package2==0.1.0",
            "package==0.1.0",
        ],
        env=env,
    )

    package = tmp_path / "bin" / exec_extension("package")
    package0_1_0 = tmp_path / "bin" / exec_extension("package0.1.0")
    package0_2_0 = tmp_path / "bin" / exec_extension("package0.2.0")
    package2 = tmp_path / "bin" / exec_extension("package2")
    assert package2.is_file()
    assert package.is_file()
    assert package0_1_0.is_file()
    assert not package0_2_0.is_file()

    # Replace the version with a "*"
    manifest = tmp_path.joinpath("manifests", "pixi-global.toml")
    manifest.write_text(manifest.read_text().replace("==0.1.0", "*"))

    verify_cli_command(
        [pixi, "global", "update", "my-packages"],
        env=env,
        stderr_contains=["- package 0.1.0 -> 0.2.0", "- package2 0.1.0 -> 0.2.0"],
    )

    assert package2.is_file()
    assert package.is_file()
    assert not package0_1_0.is_file()
    # After update be left we auto expose new binary, as all of them were exposed before
    assert package0_2_0.is_file()

    # Check the manifest for removed binaries
    manifest_content = manifest.read_text()
    assert "package0.1.0" not in manifest_content
    assert "package0.2.0" in manifest_content
    assert "package2" in manifest_content
    assert "package" in manifest_content

    # Check content of package2 file to be updated
    bin_file_package2 = tmp_path / "envs" / "my-packages" / "bin" / bat_extension("package2")
    assert "0.2.0" in bin_file_package2.read_text()


def test_pixi_update_cleanup(pixi: Path, tmp_path: Path, multiple_versions_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    package0_1_0 = tmp_path / "bin" / exec_extension("package0.1.0")
    package0_2_0 = tmp_path / "bin" / exec_extension("package0.2.0")

    verify_cli_command(
        [pixi, "global", "install", "--channel", multiple_versions_channel_1, "package==0.1.0"],
        env=env,
    )
    assert package0_1_0.is_file()
    assert not package0_2_0.is_file()

    manifest = tmp_path.joinpath("manifests", "pixi-global.toml")

    # We change the matchspec to '*'
    # Syncing shouldn't do anything
    parsed_toml = tomllib.loads(manifest.read_text())
    parsed_toml["envs"]["package"]["dependencies"]["package"] = "*"
    manifest.write_text(tomli_w.dumps(parsed_toml))
    verify_cli_command([pixi, "global", "sync"], env=env)
    assert package0_1_0.is_file()
    assert not package0_2_0.is_file()

    # Update the environment
    # The package should now have the version `0.2.0` and expose a different executable
    # The old executable should be removed
    # The new executable should be there, since user initially auto-exposed all binaries and `pixi global update` should add new binary to the manifest.
    verify_cli_command(
        [pixi, "global", "update", "package"],
        env=env,
    )
    assert not package0_1_0.is_file()
    assert package0_2_0.is_file()


def test_pixi_update_subset_expose(
    pixi: Path, tmp_path: Path, multiple_versions_channel_1: str
) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    package0_1_0 = tmp_path / "bin" / exec_extension("package0.1.0")
    package0_2_0 = tmp_path / "bin" / exec_extension("package0.2.0")

    verify_cli_command(
        [pixi, "global", "install", "--channel", multiple_versions_channel_1, "package==0.1.0"],
        env=env,
    )
    assert package0_1_0.is_file()
    assert not package0_2_0.is_file()

    manifest = tmp_path.joinpath("manifests", "pixi-global.toml")

    # We change the matchspec to '*'
    # So we expect to new binary to not be exposed,
    # since we exposed only a small subset
    parsed_toml = tomllib.loads(manifest.read_text())
    parsed_toml["envs"]["package"]["dependencies"]["package"] = "*"
    parsed_toml["envs"]["package"]["exposed"] = {"package": "package0.1.0"}

    manifest.write_text(tomli_w.dumps(parsed_toml))

    # Update the environment
    # The package should now have the version `0.2.0` and expose a different executable
    # The old executable should be removed
    # The new executable should be there, since user initially auto-exposed all binaries and `pixi global update` should add new binary to the manifest.
    verify_cli_command(
        [pixi, "global", "update", "package"],
        env=env,
    )
    assert not package0_1_0.is_file()
    assert not package0_2_0.is_file()

    # parse the manifest again
    # and check that we don't have any new binary exposed
    parsed_toml = tomllib.loads(manifest.read_text())
    assert "exposed" not in parsed_toml["envs"]["package"]


def test_auto_self_expose(pixi: Path, tmp_path: Path, non_self_expose_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    # Install jupyter and expose it as 'jupyter'
    verify_cli_command(
        [pixi, "global", "install", "--channel", non_self_expose_channel_1, "jupyter"],
        env=env,
    )
    jupyter = tmp_path / "bin" / exec_extension("jupyter")
    assert jupyter.is_file()


def test_add(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    # Can't add package to environment that doesn't exist
    verify_cli_command(
        [pixi, "global", "add", "--environment", "dummy-a", "dummy-b"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains="Environment dummy-a doesn't exist",
    )

    verify_cli_command(
        [pixi, "global", "install", "--channel", dummy_channel_1, "dummy-a"],
        env=env,
    )
    dummy_a = tmp_path / "bin" / exec_extension("dummy-a")
    assert dummy_a.is_file()

    verify_cli_command(
        [pixi, "global", "add", "--environment", "dummy-a", "dummy-b"],
        env=env,
        stderr_contains="Added package dummy-b",
    )
    # Make sure it doesn't expose a binary from this package
    dummy_b = tmp_path / "bin" / exec_extension("dummy-b")
    assert not dummy_b.is_file()

    verify_cli_command(
        [
            pixi,
            "global",
            "add",
            "--environment",
            "dummy-a",
            "dummy-b",
            "--expose",
            "dummy-b",
        ],
        env=env,
        stderr_contains=["Exposed executable dummy-b from environment dummy-a"],
    )
    # Make sure it now exposes the binary
    dummy_b = tmp_path / "bin" / exec_extension("dummy-b")
    assert dummy_b.is_file()


def test_remove_dependency(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "--environment",
            "my-env",
            "dummy-a",
            "dummy-b",
        ],
        env=env,
        stdout_contains=[
            "dependencies: dummy-a 0.1.0, dummy-b 0.1.0",
            "exposes: dummy-a, dummy-aa, dummy-b",
        ],
    )
    dummy_a = tmp_path / "bin" / exec_extension("dummy-a")
    dummy_b = tmp_path / "bin" / exec_extension("dummy-b")
    assert dummy_a.is_file()
    assert dummy_b.is_file()

    # Remove dummy-a
    verify_cli_command(
        [pixi, "global", "remove", "--environment", "my-env", "dummy-a"],
        env=env,
        stderr_contains=[
            "Removed package dummy-a in environment my-env.",
            "Removed exposed executables from environment my-env:\n   - dummy-a\n   - dummy-aa\n",
        ],
    )
    assert not dummy_a.is_file()

    # install back removed dummy-a
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "--environment",
            "my-env",
            "dummy-a",
        ],
        env=env,
    )

    verify_cli_command(
        [pixi, "global", "remove", "--environment", "my-env", "dummy-a", "dummy-b"],
        env=env,
        stderr_contains=[
            "Removed packages in environment my-env.\n    - dummy-a\n    - dummy-b",
            "Removed exposed executables from environment my-env:\n   - dummy-a\n   - dummy-aa\n   - dummy-b",
        ],
    )

    # Remove non-existing package
    verify_cli_command(
        [pixi, "global", "remove", "--environment", "my-env", "dummy-a"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains=["Dependency", "dummy-a", "not", "my-env"],
    )

    # Remove package from non-existing environment
    verify_cli_command(
        [pixi, "global", "remove", "--environment", "dummy-a", "dummy-a"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains="Environment dummy-a doesn't exist",
    )


def test_update_env_not_installed(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    original_toml = f"""
    version = {MANIFEST_VERSION}
    [envs.test]
    channels = ["{dummy_channel_1}"]
    [envs.test.dependencies]
    dummy-a = "*"
    [envs.test.exposed]
    dummy-a = "bin/dummy-a"
    """
    manifest.write_text(original_toml)
    dummy_a = tmp_path / "bin" / exec_extension("dummy-a")

    # If the environment isn't installed already,
    # `pixi global update` will install it first
    verify_cli_command(
        [pixi, "global", "update"],
        env=env,
    )
    assert dummy_a.is_file()
    # The tables in the manifest have been preserved
    assert manifest.read_text() == original_toml


@pytest.mark.parametrize(
    ("delete_exposed_on_second", "delete_env_on_second"),
    [(True, False), (False, True), (False, False)],
)
def test_update_custom_exposed_twice(
    pixi: Path,
    tmp_path: Path,
    dummy_channel_1: str,
    delete_exposed_on_second: bool,
    delete_env_on_second: bool,
) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    original_toml = f"""
    version = {MANIFEST_VERSION}
    [envs.test]
    channels = ["{dummy_channel_1}"]
    [envs.test.dependencies]
    dummy-a = "*"
    [envs.test.exposed]
    dummy-a = "bin/dummy-a"
    """
    manifest.write_text(original_toml)
    dummy_a = tmp_path / "bin" / exec_extension("dummy-a")

    # Test first update
    verify_cli_command(
        [pixi, "global", "update"],
        env=env,
    )
    assert dummy_a.is_file()
    assert manifest.read_text() == original_toml

    # Test second update
    if delete_exposed_on_second:
        dummy_a.unlink()
    if delete_env_on_second:
        shutil.rmtree(tmp_path / "envs")

    verify_cli_command(
        [pixi, "global", "update"],
        env=env,
    )
    assert dummy_a.is_file()
    assert manifest.read_text() == original_toml


def test_update_remove_old_env(
    pixi: Path,
    tmp_path: Path,
    dummy_channel_1: str,
) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()
    manifest = manifests.joinpath("pixi-global.toml")
    original_toml = f"""
    version = {MANIFEST_VERSION}
    [envs.test]
    channels = ["{dummy_channel_1}"]
    [envs.test.dependencies]
    dummy-a = "*"
    [envs.test.exposed]
    dummy-a = "bin/dummy-a"
    """
    manifest.write_text(original_toml)
    dummy_a = tmp_path / "bin" / exec_extension("dummy-a")

    # Test first update
    verify_cli_command(
        [pixi, "global", "update"],
        env=env,
    )
    assert dummy_a.is_file()
    assert manifest.read_text() == original_toml

    # Test remove env from manifest and then update
    original_toml = f"""
    version = {MANIFEST_VERSION}
    """
    manifest.write_text(original_toml)
    verify_cli_command(
        [pixi, "global", "update"],
        env=env,
    )
    assert not dummy_a.is_file()
    assert manifest.read_text() == original_toml


def test_tree(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()

    # Install dummy-a and dummy-b from dummy-channel-1
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "dummy-b==0.1.0",
            "dummy-a==0.1.0",
        ],
        env=env,
    )

    # Verify tree with dummy-b environment
    verify_cli_command(
        [pixi, "global", "tree", "--environment", "dummy-b"],
        env=env,
        stdout_contains=["dummy-b", "0.1.0"],
    )

    # Verify tree with dummy-a environment
    verify_cli_command(
        [pixi, "global", "tree", "--environment", "dummy-a"],
        env=env,
        stdout_contains=["dummy-a", "0.1.0"],
    )


def test_tree_with_filter(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()

    # Install dummy-a and dummy-b from dummy-channel-1
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "--environment",
            "dummy",
            "dummy-b==0.1.0",
            "dummy-a==0.1.0",
        ],
        env=env,
    )

    # Verify tree with regex filter for dummy environment
    verify_cli_command(
        [pixi, "global", "tree", "--environment", "dummy", "dummy-a"],
        env=env,
        stdout_contains=["dummy-a", "0.1.0"],
        stdout_excludes=["dummy-b"],
    )

    # Verify tree with regex filter for dummy-b
    verify_cli_command(
        [pixi, "global", "tree", "--environment", "dummy", "dummy-b"],
        env=env,
        stdout_contains=["dummy-b", "0.1.0"],
        stdout_excludes=["dummy-a"],
    )


def test_tree_nonexistent_environment(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    # Try to show tree for non-existent environment
    verify_cli_command(
        [pixi, "global", "tree", "--environment", "nonexistent"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains="Environment not found",
    )


def test_tree_invert(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    manifests = tmp_path.joinpath("manifests")
    manifests.mkdir()

    # Install dummy-a which has dummy-c as a dependency
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "dummy-a==0.1.0",
        ],
        env=env,
    )

    # Verify inverted tree showing what depends on dummy-c
    verify_cli_command(
        [pixi, "global", "tree", "--environment", "dummy-a", "--invert", "dummy-c"],
        env=env,
        stdout_contains=["dummy-c", "dummy-a 0.1.0"],
    )
