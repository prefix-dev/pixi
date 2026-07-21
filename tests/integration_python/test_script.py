from pathlib import Path

from .common import ExitCode, verify_cli_command


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
