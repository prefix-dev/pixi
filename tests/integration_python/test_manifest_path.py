import json
from pathlib import Path

from .common import CURRENT_PLATFORM, EMPTY_BOILERPLATE_PROJECT, ExitCode, verify_cli_command


def test_explicit_manifest_correct_location(pixi: Path, tmp_path: Path) -> None:
    current_dir = tmp_path / "current"
    target_dir = tmp_path / "target"
    current_dir.mkdir()
    target_dir.mkdir()

    (current_dir / "pixi.toml").write_text(EMPTY_BOILERPLATE_PROJECT)
    (target_dir / "pixi.toml").write_text(EMPTY_BOILERPLATE_PROJECT)

    out = verify_cli_command(
        [
            pixi,
            "shell-hook",
            "--manifest-path",
            target_dir,
            "--json",
        ],
        cwd=current_dir,
    )

    payload = json.loads(out.stdout)
    value = payload["environment_variables"].get("PIXI_PROJECT_MANIFEST")
    assert value is not None, "PIXI_PROJECT_MANIFEST missing from activated env"

    expected = (target_dir / "pixi.toml").resolve()
    actual = Path(value).resolve()
    assert actual == expected


def test_ignore_env_vars_when_manifest_path_differs(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """When running a nested pixi command with --manifest-path pointing to a different
    project, the inherited PIXI_ENVIRONMENT_NAME should be ignored because it refers
    to an environment in the parent project, not the child project.

    Scenario:
    - Parent project (at /parent) has a "test" environment with a task that runs
      `pixi run --manifest-path /child`
    - Child project (at /child) does NOT have a "test" environment
    - When the parent task runs, it sets PIXI_PROJECT_ROOT=/parent,
      PIXI_ENVIRONMENT_NAME=test, PIXI_IN_SHELL=1
    - The child pixi command should ignore PIXI_ENVIRONMENT_NAME because
      PIXI_PROJECT_ROOT differs from the child's workspace root
    """
    # Create a child project that only has the default environment (no "test" env)
    child_dir = tmp_pixi_workspace / "child"
    child_dir.mkdir()
    child_manifest = child_dir / "pixi.toml"
    child_manifest.write_text(f"""
[workspace]
name = "child-project"
channels = []
platforms = ["{CURRENT_PLATFORM}"]
""")

    # Simulate being inside a parent pixi shell by setting env vars
    # that point to a DIFFERENT project root
    different_project_root = "/some/other/project"

    # The child project root should differ from our simulated parent
    assert str(child_dir.resolve()) != different_project_root

    # Run pixi shell-hook with --manifest-path pointing to child project,
    # while env vars simulate being in a parent shell with a "test" environment.
    # shell-hook needs to select an environment, so if PIXI_ENVIRONMENT_NAME is
    # NOT ignored, this would fail with "unknown environment 'test'" since the
    # child project has no "test" env.
    verify_cli_command(
        [pixi, "shell-hook", "--manifest-path", child_manifest],
        env={
            "PIXI_PROJECT_ROOT": different_project_root,
            "PIXI_IN_SHELL": "1",
            "PIXI_ENVIRONMENT_NAME": "test",
        },
        expected_exit_code=ExitCode.SUCCESS,
    )
