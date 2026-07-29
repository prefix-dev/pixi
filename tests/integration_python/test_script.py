import json
from pathlib import Path

import pytest
from inline_snapshot import snapshot

from .common import CONDA_FORGE_CHANNEL, CURRENT_PLATFORM, ExitCode, verify_cli_command


def assert_no_workspace_state_created(workspace: Path) -> None:
    assert {path.name for path in (workspace / ".pixi").iterdir()} == {"config.toml"}


def test_pixi_init_script(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "scripts" / "example.py"
    script.parent.mkdir()
    script.write_text("#!/usr/bin/env python\nprint('hello')\n")

    verify_cli_command([pixi, "init", "--script", script, "--channel", "testing"])

    assert (
        script.read_text()
        == """#!/usr/bin/env python
#
# /// script
# requires-python = ">=3.11"
# dependencies = []
#
# [tool.pixi.workspace]
# channels = ["testing"]
# ///

print('hello')
"""
    )
    assert not (tmp_pixi_workspace / "pixi.toml").exists()
    assert_no_workspace_state_created(tmp_pixi_workspace)

    verify_cli_command(
        [pixi, "init", "--script", script],
        ExitCode.FAILURE,
        stderr_contains="already a PEP 723 script",
    )


def test_pixi_run_script_requires_inline_metadata(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "example.py"
    script.write_text("print('hello')\n")

    verify_cli_command(
        [pixi, "run", "--script", script],
        ExitCode.FAILURE,
        stderr_contains=[
            "does not contain a PEP 723 metadata block",
            "pixi init --script",
        ],
    )
    assert script.read_text() == "print('hello')\n"


def test_pixi_lock_script_requires_inline_metadata(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "example.py"
    script.write_text("print('hello')\n")

    verify_cli_command(
        [pixi, "lock", "--script", script],
        ExitCode.FAILURE,
        stderr_contains=[
            "does not contain a PEP 723 metadata block",
            "pixi init --script",
        ],
    )

    assert script.read_text() == "print('hello')\n"
    assert not script.with_name("example.py.pixi.lock").exists()


@pytest.mark.slow
def test_pixi_run_script_is_isolated_and_does_not_create_a_lock(
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
# [tool.pixi.workspace]
# channels = ["conda-forge"]
#
# [tool.pixi.dependencies]
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
        [pixi, "run", "--script", script, "first", "--second"],
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
def test_pixi_lock_script_writes_only_the_adjacent_lock(
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
# [tool.pixi.workspace]
# channels = ["{CONDA_FORGE_CHANNEL}"]
#
# [tool.pixi.dependencies]
# ///
print("hello")
'''
    )
    original_script = script.read_text()
    script_lock = script.with_name("example.py.pixi.lock")

    verify_cli_command([pixi, "lock", "--script", script, "--dry-run"], cwd=tmp_pixi_workspace)
    assert script.read_text() == original_script
    assert not script_lock.exists()

    verify_cli_command([pixi, "lock", "--script", script], cwd=tmp_pixi_workspace)
    assert script.read_text() == original_script
    assert script_lock.exists()
    assert not (tmp_pixi_workspace / "pixi.lock").exists()
    assert_no_workspace_state_created(tmp_pixi_workspace)


def test_pixi_add_script_requires_inline_metadata(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "example.py"
    script.write_text("print('hello')\n")

    verify_cli_command(
        [pixi, "add", "--script", script, "rich"],
        ExitCode.FAILURE,
        stderr_contains=[
            "does not contain a PEP 723 metadata block",
            "pixi init --script",
        ],
    )

    assert script.read_text() == "print('hello')\n"
    assert not script.with_name("example.py.pixi.lock").exists()


@pytest.mark.slow
def test_pixi_add_script_writes_conda_and_pypi_dependencies(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    script = tmp_pixi_workspace / "example.py"
    script.write_text(
        f'''# /// script
# requires-python = ">=3.11"
# dependencies = []
#
# [tool.pixi.workspace]
# channels = ["{CONDA_FORGE_CHANNEL}"]
#
# [tool.uv]
# prerelease = "allow"
# ///
print("hello")
'''
    )
    script_lock = script.with_name("example.py.pixi.lock")

    verify_cli_command(
        [pixi, "add", "--script", script, "--no-install", "bzip2"],
        cwd=tmp_pixi_workspace,
        stderr_contains="Added bzip2",
    )
    assert not script_lock.exists()

    verify_cli_command([pixi, "lock", "--script", script], cwd=tmp_pixi_workspace)
    original_lock = script_lock.read_text()

    verify_cli_command(
        [
            pixi,
            "add",
            "--script",
            script,
            "--no-install",
            "--pypi",
            "requests==2.32.5",
        ],
        cwd=tmp_pixi_workspace,
        stderr_contains="Added requests==2.32.5",
    )

    assert script.read_text() == snapshot(
        f'''# /// script
# requires-python = ">=3.11"
# dependencies = ["requests==2.32.5"]
#
# [tool.pixi.workspace]
# channels = ["{CONDA_FORGE_CHANNEL}"]
#
# [tool.pixi.dependencies]
# bzip2 = "*"
#
# [tool.uv]
# prerelease = "allow"
# ///
print("hello")
'''
    )
    assert script_lock.read_text() != original_lock
    assert not (tmp_pixi_workspace / "pixi.lock").exists()
    assert_no_workspace_state_created(tmp_pixi_workspace)


def test_pixi_remove_script_requires_inline_metadata(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "example.py"
    script.write_text("print('hello')\n")

    verify_cli_command(
        [pixi, "remove", "--script", script, "requests"],
        ExitCode.FAILURE,
        stderr_contains=[
            "does not contain a PEP 723 metadata block",
            "pixi init --script",
        ],
    )
    assert script.read_text() == "print('hello')\n"


@pytest.mark.slow
def test_pixi_remove_script_uses_explicit_ecosystem(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "example.py"
    script.write_text(
        f'''# /// script
# dependencies = ["requests==2.32.5"]
#
# [tool.pixi.workspace]
# channels = ["{CONDA_FORGE_CHANNEL}"]
#
# [tool.pixi.dependencies]
# bzip2 = "*"
#
# [tool.uv]
# prerelease = "allow"
# ///
print("hello")
'''
    )
    script_lock = script.with_name("example.py.pixi.lock")

    verify_cli_command(
        [pixi, "remove", "--script", script, "--no-install", "bzip2"],
        stderr_contains="Removed bzip2",
    )
    assert not script_lock.exists()

    verify_cli_command([pixi, "lock", "--script", script], cwd=tmp_pixi_workspace)
    original_lock = script_lock.read_text()

    verify_cli_command(
        [pixi, "remove", "--script", script, "--no-install", "--pypi", "requests"],
        stderr_contains="Removed requests",
    )

    assert script.read_text() == snapshot(
        f'''# /// script
# dependencies = []
#
# [tool.pixi.workspace]
# channels = ["{CONDA_FORGE_CHANNEL}"]
#
# [tool.uv]
# prerelease = "allow"
# ///
print("hello")
'''
    )
    assert script_lock.read_text() != original_lock
    assert not (tmp_pixi_workspace / "pixi.lock").exists()
    assert_no_workspace_state_created(tmp_pixi_workspace)
