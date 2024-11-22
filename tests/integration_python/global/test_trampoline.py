import json
import pathlib
from pathlib import Path
import platform

from ..common import verify_cli_command, exec_extension, is_binary


def test_trampoline_respect_activation_variables(
    pixi: Path, tmp_path: Path, trampoline_channel_1: str
) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    dummy_b = tmp_path / "bin" / exec_extension("dummy-trampoline")

    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            trampoline_channel_1,
            "dummy-trampoline",
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
    assert "PATH" in trampoline_env

    # verify that exe and root folder is correctly set to the original one
    original_dummy_b = tmp_path / "envs" / "dummy-trampoline" / "bin" / "dummy-trampoline"
    if platform.system() == "Windows":
        original_dummy_b = original_dummy_b.with_suffix(".bat")
    assert pathlib.Path(trampoline_metadata["exe"]) == pathlib.Path(original_dummy_b)
    assert trampoline_metadata["path"] == str(original_dummy_b.parent)

    # now execute the binary
    verify_cli_command([dummy_b], stdout_contains="Success:")


def test_trampoline_new_activation_scripts(
    pixi: Path, tmp_path: Path, trampoline_channel_1: str, trampoline_channel_2: str
) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    dummy_b = tmp_path / "bin" / exec_extension("dummy-trampoline")

    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            trampoline_channel_1,
            "dummy-trampoline==0.1.0",
        ],
        env=env,
    )

    assert is_binary(dummy_b)

    dummy_b_json = tmp_path / "bin" / "trampoline_configuration" / "dummy-trampoline.json"

    trampoline_metadata = json.loads(dummy_b_json.read_text())

    # get envs of the trampoline
    assert trampoline_metadata["env"]["TRAMPOLINE_TEST_ENV"] == "teapot"

    # now install newever version that have different activation scripts

    # Replace the version with a "*"
    manifest = tmp_path.joinpath("manifests", "pixi-global.toml")
    manifest.write_text(manifest.read_text().replace("trampoline_1", "trampoline_2"))

    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--force-reinstall",
            "dummy-trampoline==0.2.0",
        ],
        env=env,
    )

    # verify that newever activation is recorded
    dummy_b_json = tmp_path / "bin" / "trampoline_configuration" / "dummy-trampoline.json"

    trampoline_metadata = json.loads(dummy_b_json.read_text())

    # get envs of the trampoline
    assert trampoline_metadata["env"]["TRAMPOLINE_V2_TEST_ENV"] == "teapot_v2"
    # verify that older env is not present
    assert "TRAMPOLINE_TEST_ENV" not in trampoline_metadata["env"]

    # now execute the binary
    verify_cli_command([dummy_b], stdout_contains="Success:")


def test_trampoline_migrate_previous_script(
    pixi: Path, tmp_path: Path, trampoline_channel_1: str
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
            trampoline_channel_1,
            "dummy-trampoline",
        ],
        env=env,
    )

    assert dummy_trampoline.is_file()
    assert is_binary(dummy_trampoline)

    dummy_trampoline_json = tmp_path / "bin" / "trampoline_configuration" / "dummy-trampoline.json"

    assert dummy_trampoline_json.is_file()


def test_trampoline_dot_in_exe(pixi: Path, tmp_path: Path, trampoline_channel_1: str) -> None:
    env = {"PIXI_HOME": str(tmp_path)}

    # Expose binary with a dot in the name
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            trampoline_channel_1,
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
    pixi: Path, tmp_path: Path, trampoline_channel_1: str
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
            trampoline_channel_1,
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
