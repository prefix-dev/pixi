import copy
import json
import os
import pathlib
import platform
from pathlib import Path
from typing import Any

import pytest

from ..common import exec_extension, is_binary, verify_cli_command


def break_configuration(configuration_path: Path) -> Any:
    """Break trampoline configuration by removing `path_diff`"""
    configuration = json.loads(configuration_path.read_text())
    original_configuration = copy.deepcopy(configuration)
    del configuration["path_diff"]
    configuration_path.write_text(json.dumps(configuration))
    return original_configuration


def test_trampoline_respect_activation_variables(
    pixi: Path, tmp_path: Path, trampoline_channel: str
) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    dummy_b = tmp_path / "bin" / exec_extension("dummy-trampoline")

    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            trampoline_channel,
            "dummy-trampoline==0.1.0",
        ],
        env=env,
    )

    assert is_binary(dummy_b)

    dummy_b_json = tmp_path / "bin" / "trampoline_configuration" / "dummy-trampoline.json"

    trampoline_metadata = json.loads(dummy_b_json.read_text())

    # get envs of the trampoline
    trampoline_env = trampoline_metadata["env"]
    assert trampoline_env["TRAMPOLINE_TEST_ENV"] == "teapot"
    assert "CONDA_PREFIX" in trampoline_env
    assert "PATH" not in trampoline_env

    # verify that exe and root folder is correctly set to the original one
    original_dummy_b = tmp_path / "envs" / "dummy-trampoline" / "bin" / "dummy-trampoline"
    if platform.system() == "Windows":
        original_dummy_b = original_dummy_b.with_suffix(".bat")
    assert pathlib.Path(trampoline_metadata["exe"]) == pathlib.Path(original_dummy_b)

    # now execute the binary
    verify_cli_command([dummy_b], stdout_contains="Success:")


def test_trampoline_new_activation_scripts(
    pixi: Path, tmp_path: Path, trampoline_channel: str
) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    dummy_b = tmp_path / "bin" / exec_extension("dummy-trampoline")

    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            trampoline_channel,
            "dummy-trampoline==0.1.0",
        ],
        env=env,
    )

    assert is_binary(dummy_b)

    dummy_b_json = tmp_path / "bin" / "trampoline_configuration" / "dummy-trampoline.json"

    trampoline_metadata = json.loads(dummy_b_json.read_text())

    # get envs of the trampoline
    assert trampoline_metadata["env"]["TRAMPOLINE_TEST_ENV"] == "teapot"

    # now install newer version that have different activation scripts
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "dummy-trampoline==0.2.0",
        ],
        env=env,
    )

    # verify that newer activation is recorded
    dummy_b_json = tmp_path / "bin" / "trampoline_configuration" / "dummy-trampoline.json"

    trampoline_metadata = json.loads(dummy_b_json.read_text())

    # get envs of the trampoline
    assert trampoline_metadata["env"]["TRAMPOLINE_V2_TEST_ENV"] == "teapot_v2"
    # verify that older env is not present
    assert "TRAMPOLINE_TEST_ENV" not in trampoline_metadata["env"]

    # now execute the binary
    verify_cli_command([dummy_b], stdout_contains="Success:")


def test_trampoline_migrate_previous_script(
    pixi: Path, tmp_path: Path, trampoline_channel: str
) -> None:
    # this test will validate if new trampoline will migrate the previous way of running packages (using scripts)
    env = {"PIXI_HOME": str(tmp_path)}

    # create a dummy script that will act as already installed package
    dummy_trampoline = tmp_path / "bin" / exec_extension("dummy-trampoline")

    # now run install again, this time it should migrate the script to the new trampoline
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            trampoline_channel,
            "dummy-trampoline",
        ],
        env=env,
    )

    assert dummy_trampoline.is_file()
    assert is_binary(dummy_trampoline)

    dummy_trampoline_json = tmp_path / "bin" / "trampoline_configuration" / "dummy-trampoline.json"

    assert dummy_trampoline_json.is_file()


def test_trampoline_dot_in_exe(pixi: Path, tmp_path: Path, trampoline_channel: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    # Expose binary with a dot in the name
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            trampoline_channel,
            "dummy-trampoline",
            "--expose",
            "exe.test=dummy-trampoline",
        ],
        env=env,
    )

    exe_test = tmp_path / "bin" / exec_extension("exe.test")
    # The binary execute should succeed
    verify_cli_command([exe_test], stdout_contains="Success:")


def test_trampoline_migrate_with_newer_trampoline(
    pixi: Path, tmp_path: Path, trampoline_channel: str
) -> None:
    # this test will validate if new trampoline will migrate the older trampoline
    env = {"PIXI_HOME": str(tmp_path)}

    # create a dummy bin that will act as already installed package
    dummy_trampoline = tmp_path / "bin" / exec_extension("dummy-trampoline")
    dummy_trampoline.parent.mkdir(exist_ok=True)
    dummy_trampoline.write_text("hello")

    # now run install again, this time it should migrate the script to the new trampoline
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            trampoline_channel,
            "dummy-trampoline",
        ],
        env=env,
    )

    assert dummy_trampoline.is_file()
    assert is_binary(dummy_trampoline)

    dummy_trampoline_json = tmp_path / "bin" / "trampoline_configuration" / "dummy-trampoline.json"

    assert dummy_trampoline_json.is_file()
    # run an update, it should say that everything is up to date
    verify_cli_command(
        [
            pixi,
            "global",
            "update",
        ],
        env=env,
        stderr_contains="Environment dummy-trampoline was already up-to-date",
        stderr_excludes="Updated executable dummy-trampoline of environment dummy-trampoline",
    )

    # now change the trampoline binary , and verify that it will install the new one
    dummy_trampoline.write_text("new content")

    # run an update again it should remove the old trampoline and install the new one
    verify_cli_command(
        [
            pixi,
            "global",
            "update",
        ],
        env=env,
        stderr_contains="Updated executable dummy-trampoline of environment dummy-trampoline",
    )

    # run an update again
    verify_cli_command(
        [
            pixi,
            "global",
            "update",
        ],
        env=env,
        stderr_contains="Environment dummy-trampoline was already up-to-date",
        stderr_excludes="Updated executable dummy-trampoline of environment dummy-trampoline",
    )


def test_trampoline_install_should_only_migrate_own_environment(
    pixi: Path, tmp_path: Path, trampoline_channel: str
) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    # now run install again, this time it should migrate the script to the new trampoline
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            trampoline_channel,
            "dummy-trampoline",
            "dummy-trampoline-2",
        ],
        env=env,
    )

    dummy_trampoline = tmp_path / "bin" / exec_extension("dummy-trampoline")
    dummy_trampoline_2 = tmp_path / "bin" / exec_extension("dummy-trampoline-2")
    dummy_trampoline_json = tmp_path / "bin" / "trampoline_configuration" / "dummy-trampoline.json"
    dummy_trampoline_2_json = (
        tmp_path / "bin" / "trampoline_configuration" / "dummy-trampoline-2.json"
    )

    assert dummy_trampoline.is_file()
    assert is_binary(dummy_trampoline)
    assert dummy_trampoline_2.is_file()
    assert is_binary(dummy_trampoline_2)
    assert dummy_trampoline_2.read_bytes() == dummy_trampoline.read_bytes()
    assert dummy_trampoline_json.is_file()
    assert dummy_trampoline_2_json.is_file()

    original_trampoline = dummy_trampoline.read_bytes()

    # Break both dummy-trampoline and dummy-trampoline-2
    # Since they are hardlinked, they have now the same content
    broken_trampoline = b"\x00\x01\x02\x03\x04"
    dummy_trampoline.write_bytes(broken_trampoline)

    original_dummy_trampoline_configuration = break_configuration(dummy_trampoline_json)
    original_dummy_trampoline_2_configuration = break_configuration(dummy_trampoline_2_json)
    broken_dummy_trampoline_2_configuration = json.loads(dummy_trampoline_2_json.read_text())

    # Install "dummy-trampoline" package, this should update its trampoline and configuration
    # However, it shouldn't touch trampoline nor configuration of dummy-trampoline-2
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            trampoline_channel,
            "dummy-trampoline",
        ],
        env=env,
    )
    assert dummy_trampoline.read_bytes() == original_trampoline
    assert dummy_trampoline_2.read_bytes() == broken_trampoline
    assert json.loads(dummy_trampoline_json.read_text()) == original_dummy_trampoline_configuration
    assert (
        json.loads(dummy_trampoline_2_json.read_text()) == broken_dummy_trampoline_2_configuration
    )

    # run sync, all trampolines and configurations should be updated
    verify_cli_command(
        [
            pixi,
            "global",
            "sync",
        ],
        env=env,
    )

    assert dummy_trampoline.read_bytes() == original_trampoline
    assert dummy_trampoline_2.read_bytes() == original_trampoline
    assert json.loads(dummy_trampoline_json.read_text()) == original_dummy_trampoline_configuration
    assert (
        json.loads(dummy_trampoline_2_json.read_text()) == original_dummy_trampoline_2_configuration
    )


def test_trampoline_migrate_with_newer_configuration(
    pixi: Path, tmp_path: Path, trampoline_channel: str
) -> None:
    # this test will validate if new trampoline will migrate the older trampoline
    env = {"PIXI_HOME": str(tmp_path)}

    # create a dummy bin that will act as already installed package
    dummy_trampoline = tmp_path / "bin" / exec_extension("dummy-trampoline")
    dummy_trampoline.parent.mkdir(exist_ok=True)
    dummy_trampoline.write_text("hello")

    # now run install again, this time it should migrate the script to the new trampoline
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            trampoline_channel,
            "dummy-trampoline",
        ],
        env=env,
    )

    assert dummy_trampoline.is_file()
    assert is_binary(dummy_trampoline)

    dummy_trampoline_json = tmp_path / "bin" / "trampoline_configuration" / "dummy-trampoline.json"

    assert dummy_trampoline_json.is_file()
    # run an update, it should say that everything is up to date
    verify_cli_command(
        [
            pixi,
            "global",
            "update",
        ],
        env=env,
        stderr_contains="Environment dummy-trampoline was already up-to-date",
        stderr_excludes="Updated executable dummy-trampoline of environment dummy-trampoline",
    )

    original_configuration = break_configuration(dummy_trampoline_json)

    # run an update again it should remove the modified configuration and install the valid one again
    verify_cli_command(
        [
            pixi,
            "global",
            "update",
        ],
        env=env,
    )
    assert json.loads(dummy_trampoline_json.read_text()) == original_configuration

    # now change the trampoline binary and configuration at the same time
    dummy_trampoline.write_text("new content")
    original_configuration = break_configuration(dummy_trampoline_json)
    verify_cli_command(
        [
            pixi,
            "global",
            "update",
        ],
        env=env,
        stderr_contains="Updated executable dummy-trampoline of environment dummy-trampoline",
    )


@pytest.mark.parametrize(
    ("extend_path_prefix_entry"),
    [False, True],
)
def test_trampoline_extends_path(
    pixi: Path,
    tmp_path: Path,
    trampoline_path_channel: str,
    extend_path_prefix_entry: bool,
) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    dummy_trampoline_path = tmp_path / "bin" / exec_extension("dummy-trampoline-path")

    original_path = os.environ["PATH"]
    env["PATH"] = original_path
    path_diff = "/test/path"

    if extend_path_prefix_entry:
        # Extend PATH with prefix entry
        prefix_bin = str(tmp_path.joinpath("envs", "dummy-trampoline-path", "bin"))
        env["PATH"] = os.pathsep.join([prefix_bin, original_path])

    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            trampoline_path_channel,
            "dummy-trampoline-path",
        ],
        env=env,
    )

    if extend_path_prefix_entry:
        dummy_trampoline_path_configuration = (
            tmp_path / "bin" / "trampoline_configuration" / "dummy-trampoline-path.json"
        )
        trampoline_metadata = json.loads(dummy_trampoline_path_configuration.read_text())
        assert prefix_bin in trampoline_metadata["path_diff"]

    # PATH should be extended by the activation script
    # This is done by adding the diff before and after the activation script to the current PATH
    env["PATH"] = original_path
    verify_cli_command([dummy_trampoline_path], stdout_contains=[path_diff, env["PATH"]], env=env)

    # If we extend PATH, both new extension and path diff should be present
    path_change = "/another/test/path"
    env["PATH"] = os.pathsep.join([path_change, original_path])
    verify_cli_command([dummy_trampoline_path], stdout_contains=[path_diff, env["PATH"]], env=env)

    # If we set PIXI_BASE_PATH, the order will be different
    parts = env["PATH"].split(os.pathsep)
    extra_parts = parts[0]
    base_path = os.pathsep.join(parts[1:])
    env["PIXI_BASE_PATH"] = base_path
    verify_cli_command(
        [dummy_trampoline_path], stdout_contains=[extra_parts, path_diff, base_path], env=env
    )


def test_trampoline_removes_trampolines_not_in_manifest(
    pixi: Path, tmp_path: Path, trampoline_channel: str
) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    dummy_trampoline_original = tmp_path / "bin" / exec_extension("dummy-trampoline")

    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            trampoline_channel,
            "dummy-trampoline",
        ],
        env=env,
    )

    dummy_trampoline_new = dummy_trampoline_original.rename(
        dummy_trampoline_original.parent / exec_extension("dummy-trampoline-new")
    )

    verify_cli_command([pixi, "global", "sync"], env=env)
    assert dummy_trampoline_original.is_file()
    assert not dummy_trampoline_new.is_file()
