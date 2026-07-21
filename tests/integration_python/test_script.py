import json
from pathlib import Path

import pytest

from .common import CURRENT_PLATFORM, ExitCode, verify_cli_command


def assert_no_workspace_state_created(workspace: Path) -> None:
    assert {path.name for path in (workspace / ".pixi").iterdir()} == {"config.toml"}


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


def test_pixi_script_run_requires_inline_metadata(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "example.py"
    script.write_text("print('hello')\n")

    verify_cli_command(
        [pixi, "script", "run", script],
        ExitCode.FAILURE,
        stderr_contains=[
            "does not contain a PEP 723 metadata block",
            "pixi script init",
        ],
    )
    assert script.read_text() == "print('hello')\n"


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
