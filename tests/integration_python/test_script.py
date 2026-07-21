import json
import tomllib
from pathlib import Path

import pytest

from .common import CONDA_FORGE_CHANNEL, CURRENT_PLATFORM, ExitCode, verify_cli_command


def assert_no_workspace_state_created(workspace: Path) -> None:
    assert {path.name for path in (workspace / ".pixi").iterdir()} == {"config.toml"}


def read_script_metadata(script: Path) -> dict:
    lines = script.read_text().splitlines()
    opening = lines.index("# /// script")
    closing = lines.index("# ///", opening + 1)
    return tomllib.loads(
        "\n".join(
            line.removeprefix("# ") if line != "#" else "" for line in lines[opening + 1 : closing]
        )
    )


def test_pixi_script_init(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "scripts" / "example.py"
    script.parent.mkdir()
    script.write_text("#!/usr/bin/env python\nprint('hello')\n")

    verify_cli_command([pixi, "script", "init", script, "--channel", "testing"])

    assert (
        script.read_text()
        == """#!/usr/bin/env python
#
# /// script
# requires-python = ">=3.11"
# dependencies = []
#
# [tool.conda]
# channels = ["testing"]
# dependencies = []
# ///

print('hello')
"""
    )
    assert not (tmp_pixi_workspace / "pixi.toml").exists()
    assert_no_workspace_state_created(tmp_pixi_workspace)

    verify_cli_command(
        [pixi, "script", "init", script],
        ExitCode.FAILURE,
        stderr_contains="already a PEP 723 script",
    )


@pytest.mark.parametrize(
    ("subcommand", "extra_args"),
    [("run", []), ("lock", []), ("remove", ["requests"])],
)
def test_pixi_script_commands_require_inline_metadata(
    pixi: Path, tmp_pixi_workspace: Path, subcommand: str, extra_args: list[str]
) -> None:
    script = tmp_pixi_workspace / "example.py"
    script.write_text("print('hello')\n")

    verify_cli_command(
        [pixi, "script", subcommand, script, *extra_args],
        ExitCode.FAILURE,
        stderr_contains=[
            "does not contain a PEP 723 metadata block",
            "pixi script init",
        ],
    )

    assert script.read_text() == "print('hello')\n"
    assert not script.with_name("example.py.pixi.lock").exists()


@pytest.mark.parametrize(
    ("subcommand", "extra_args"),
    [("run", []), ("lock", []), ("add", ["rich"]), ("remove", ["requests"])],
)
def test_pixi_script_commands_require_an_existing_file(
    pixi: Path, tmp_pixi_workspace: Path, subcommand: str, extra_args: list[str]
) -> None:
    # Only `pixi script init` creates new files; a typo'd path must not
    # produce a file or an environment.
    script = tmp_pixi_workspace / "missing.py"

    verify_cli_command(
        [pixi, "script", subcommand, script, *extra_args],
        ExitCode.FAILURE,
        stderr_contains="does not exist",
    )

    assert not script.exists()
    assert not script.with_name("missing.py.pixi.lock").exists()


@pytest.mark.slow
def test_pixi_script_run_is_isolated_and_does_not_create_a_lock(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    (tmp_pixi_workspace / "pixi.toml").write_text(
        f'''[workspace]
name = "enclosing"
channels = []
platforms = ["{CURRENT_PLATFORM}"]
'''
    )
    script = tmp_pixi_workspace / "scripts" / "example.py"
    script.parent.mkdir()
    script.write_text(
        """# /// script
# requires-python = ">=3.11"
# dependencies = []
#
# [tool.conda]
# channels = ["conda-forge"]
# dependencies = []
# ///
import json
import os
import sys

print(json.dumps({
    "argv": sys.argv[1:],
    "cwd": os.getcwd(),
    "manifest": os.environ["PIXI_PROJECT_MANIFEST"],
}))
"""
    )

    verify_cli_command(
        [pixi, "script", "run", script, "first", "--second"],
        cwd=tmp_pixi_workspace,
        env={
            "PIXI_PROJECT_ROOT": str(tmp_pixi_workspace),
            "PIXI_ENVIRONMENT_NAME": "ignored",
        },
        stdout_contains=json.dumps(
            {
                "argv": ["first", "--second"],
                "cwd": str(tmp_pixi_workspace),
                "manifest": str(script),
            }
        ),
    )

    assert not script.with_name("example.py.pixi.lock").exists()
    assert_no_workspace_state_created(tmp_pixi_workspace)


@pytest.mark.slow
def test_pixi_script_lock_writes_only_the_adjacent_lock(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    (tmp_pixi_workspace / "pixi.toml").write_text(
        f'''[workspace]
name = "enclosing"
channels = []
platforms = ["{CURRENT_PLATFORM}"]
'''
    )
    script = tmp_pixi_workspace / "scripts" / "example.py"
    script.parent.mkdir()
    script.write_text(
        f'''# /// script
# requires-python = ">=3.11"
# dependencies = []
#
# [tool.conda]
# channels = ["{CONDA_FORGE_CHANNEL}"]
# dependencies = []
# ///
print("hello")
'''
    )
    original_script = script.read_text()
    script_lock = script.with_name("example.py.pixi.lock")

    verify_cli_command([pixi, "script", "lock", "--dry-run", script], cwd=tmp_pixi_workspace)
    assert script.read_text() == original_script
    assert not script_lock.exists()

    verify_cli_command([pixi, "script", "lock", script], cwd=tmp_pixi_workspace)
    assert script.read_text() == original_script
    assert script_lock.exists()
    assert not (tmp_pixi_workspace / "pixi.lock").exists()
    assert_no_workspace_state_created(tmp_pixi_workspace)


@pytest.mark.slow
def test_pixi_script_add_initializes_and_uses_portable_dependency_locations(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    script = tmp_pixi_workspace / "example.py"
    script.write_text("print('hello')\n")

    verify_cli_command(
        [pixi, "script", "add", "--no-install", script, "rich"],
        cwd=tmp_pixi_workspace,
    )
    verify_cli_command(
        [
            pixi,
            "script",
            "add",
            "--no-install",
            "--pypi",
            script,
            "requests==2.32.5",
        ],
        cwd=tmp_pixi_workspace,
    )

    contents = script.read_text()
    metadata = read_script_metadata(script)

    assert contents.endswith("print('hello')\n")
    assert metadata["dependencies"] == ["requests==2.32.5"]
    assert metadata["tool"]["conda"]["channels"] == [CONDA_FORGE_CHANNEL]
    assert any(spec.split()[0] == "rich" for spec in metadata["tool"]["conda"]["dependencies"])
    assert "pixi" not in metadata["tool"]
    assert not script.with_name("example.py.pixi.lock").exists()


@pytest.mark.slow
def test_pixi_script_remove_infers_conda_and_pypi(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "example.py"
    script.write_text(
        f'''# /// script
# dependencies = ["requests==2.32.5"]
#
# [tool.conda]
# channels = ["{CONDA_FORGE_CHANNEL}"]
# dependencies = ["rich"]
#
# [tool.uv]
# prerelease = "allow"
# ///
print("hello")
'''
    )

    verify_cli_command([pixi, "script", "remove", "--no-install", script, "rich"])
    verify_cli_command([pixi, "script", "remove", "--no-install", script, "requests"])

    contents = script.read_text()
    metadata = read_script_metadata(script)

    assert contents.endswith('print("hello")\n')
    assert metadata["dependencies"] == []
    assert metadata["tool"]["conda"]["dependencies"] == []
    assert metadata["tool"]["uv"] == {"prerelease": "allow"}
    assert not script.with_name("example.py.pixi.lock").exists()
