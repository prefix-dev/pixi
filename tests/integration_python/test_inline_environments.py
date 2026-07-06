import json
import os
import sys
import time
import tomllib
from pathlib import Path

import pytest

from .common import CURRENT_PLATFORM, ExitCode, verify_cli_command


def test_inline_environment_installs_and_runs(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = f"""
    [workspace]
    name = "test"
    channels = ["{dummy_channel_1}"]
    platforms = ["{CURRENT_PLATFORM}"]

    [environments.dev]
    dependencies = {{ dummy-a = "*" }}

    [environments.dev.tasks]
    greet = "echo hello-from-dev"
    """
    manifest.write_text(toml)

    # The inline task runs in the inline environment.
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "--environment", "dev", "greet"],
        stdout_contains="hello-from-dev",
    )

    # The inline dependency is installed in the environment.
    verify_cli_command(
        [pixi, "list", "--manifest-path", manifest, "--environment", "dev"],
        stdout_contains="dummy-a",
    )


def test_inline_environment_combines_with_features(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = f"""
    [workspace]
    name = "test"
    channels = ["{dummy_channel_1}"]
    platforms = ["{CURRENT_PLATFORM}"]

    [feature.shared.dependencies]
    dummy-b = "*"

    [environments.dev]
    features = ["shared"]
    dependencies = {{ dummy-a = "*" }}
    """
    manifest.write_text(toml)

    verify_cli_command(
        [pixi, "list", "--manifest-path", manifest, "--environment", "dev"],
        stdout_contains=["dummy-a", "dummy-b"],
    )


def test_reserved_env_feature_name_rejected(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = f"""
    [workspace]
    name = "test"
    channels = ["{dummy_channel_1}"]
    platforms = ["{CURRENT_PLATFORM}"]

    [environments.dev]
    dependencies = {{ dummy-a = "*" }}
    """
    manifest.write_text(toml)

    # The reserved feature namespace is rejected on the CLI.
    verify_cli_command(
        [pixi, "add", "--manifest-path", manifest, "--feature", "env:dev", "dummy-b"],
        ExitCode.INCORRECT_USAGE,
        stderr_contains="reserved",
    )

    # Referencing the synthesized feature of another environment is rejected.
    toml += """
    [environments.other]
    features = ["env:dev"]
    """
    manifest.write_text(toml)
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest, "--environment", "other"],
        ExitCode.FAILURE,
        stderr_contains="cannot be referenced",
    )


def workspace_header(channel: str) -> str:
    return f"""
    [workspace]
    name = "test"
    channels = ["{channel}"]
    platforms = ["{CURRENT_PLATFORM}"]
    """


def test_inline_task_runs_without_environment_flag(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = (
        workspace_header(dummy_channel_1)
        + """
    [environments.dev.tasks]
    greet = "echo hello-from-dev"
    """
    )
    manifest.write_text(toml)

    # The task is only defined in the inline environment, so pixi should pick
    # that environment without an explicit `--environment`.
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "greet"],
        stdout_contains="hello-from-dev",
    )


def test_inline_task_listed(pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = (
        workspace_header(dummy_channel_1)
        + """
    [environments.dev.tasks]
    greet = "echo hello-from-dev"
    """
    )
    manifest.write_text(toml)

    # `pixi task list` prints the task names to stderr.
    verify_cli_command(
        [pixi, "task", "list", "--manifest-path", manifest],
        stderr_contains="greet",
    )


def test_inline_activation_env(pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = (
        workspace_header(dummy_channel_1)
        + """
    [environments.dev.activation.env]
    INLINE_VAR = "inline-var-value"

    [environments.dev.tasks]
    show = "echo $INLINE_VAR"
    """
    )
    manifest.write_text(toml)

    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "--environment", "dev", "show"],
        stdout_contains="inline-var-value",
    )


def test_inline_channels(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str, dummy_channel_2: str
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = (
        workspace_header(dummy_channel_1)
        + f"""
    [environments.dev]
    channels = ["{dummy_channel_2}"]
    dependencies = {{ dummy-x = "*" }}
    """
    )
    manifest.write_text(toml)

    # dummy-x only exists in dummy_channel_2, which is added inline.
    verify_cli_command(
        [pixi, "list", "--manifest-path", manifest, "--environment", "dev"],
        stdout_contains="dummy-x",
    )


def test_inline_platforms(pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str) -> None:
    other_platform = "osx-64" if CURRENT_PLATFORM != "osx-64" else "linux-64"
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = f"""
    [workspace]
    name = "test"
    channels = ["{dummy_channel_1}"]
    platforms = ["{CURRENT_PLATFORM}", "{other_platform}"]

    [environments.dev]
    platforms = ["{CURRENT_PLATFORM}"]
    dependencies = {{ dummy-a = "*" }}
    """
    manifest.write_text(toml)

    verify_cli_command([pixi, "install", "--manifest-path", manifest, "--environment", "dev"])

    # The lock file should only contain the restricted platform for dev.
    info = verify_cli_command([pixi, "info", "--json", "--manifest-path", manifest])
    data = json.loads(info.stdout)
    dev = next(env for env in data["environments_info"] if env["name"] == "dev")
    assert [platform["name"] for platform in dev["platforms"]] == [CURRENT_PLATFORM]


def test_inline_target_dependencies(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = (
        workspace_header(dummy_channel_1)
        + f"""
    [environments.dev.target.{CURRENT_PLATFORM}.dependencies]
    dummy-b = "*"
    """
    )
    manifest.write_text(toml)

    verify_cli_command(
        [pixi, "list", "--manifest-path", manifest, "--environment", "dev"],
        stdout_contains="dummy-b",
    )


def test_inline_constraints(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = (
        workspace_header(multiple_versions_channel_1)
        + """
    [environments.dev]
    dependencies = { package3 = "*" }
    constraints = { package3 = "<0.3.0" }
    """
    )
    manifest.write_text(toml)

    verify_cli_command(
        [pixi, "list", "--manifest-path", manifest, "--environment", "dev"],
        stdout_contains="0.2.0",
        stdout_excludes="0.3.0",
    )


def test_inline_solve_group(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = (
        workspace_header(multiple_versions_channel_1)
        + """
    [environments.dev]
    dependencies = { package = "==0.1.0" }
    solve-group = "group"

    [environments.test]
    dependencies = { package = "*" }
    solve-group = "group"
    """
    )
    manifest.write_text(toml)

    # Because both environments are in the same solve group, the unpinned
    # environment should also get the pinned version.
    verify_cli_command(
        [pixi, "list", "--manifest-path", manifest, "--environment", "test"],
        stdout_contains="0.1.0",
        stdout_excludes="0.2.0",
    )


def test_inline_no_default_feature(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = (
        workspace_header(dummy_channel_1)
        + """
    [dependencies]
    dummy-a = "*"

    [environments.dev]
    dependencies = { dummy-b = "*" }
    no-default-feature = true
    """
    )
    manifest.write_text(toml)

    verify_cli_command(
        [pixi, "list", "--manifest-path", manifest, "--environment", "dev"],
        stdout_contains="dummy-b",
        stdout_excludes="dummy-a",
    )


def test_pyproject_inline_environment(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pyproject.toml")
    toml = f"""
    [project]
    name = "test"
    version = "0.1.0"

    [tool.pixi.workspace]
    channels = ["{dummy_channel_1}"]
    platforms = ["{CURRENT_PLATFORM}"]

    # The default feature of a pyproject manifest implicitly depends on
    # python, which the dummy channel does not provide, so avoid solving.
    [tool.pixi.environments.dev]
    dependencies = {{ dummy-a = "*" }}
    no-default-feature = true
    """
    manifest.write_text(toml)

    info = verify_cli_command([pixi, "info", "--json", "--manifest-path", manifest])
    data = json.loads(info.stdout)
    dev = next(env for env in data["environments_info"] if env["name"] == "dev")
    assert dev["features"] == ["env:dev"]
    assert dev["dependencies"] == ["dummy-a"]


def test_task_add_to_env_feature_rejected(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = (
        workspace_header(dummy_channel_1)
        + """
    [environments.dev.dependencies]
    dummy-a = "*"
    """
    )
    manifest.write_text(toml)

    verify_cli_command(
        [
            pixi,
            "task",
            "add",
            "--manifest-path",
            manifest,
            "--feature",
            "env:dev",
            "greet",
            "echo hi",
        ],
        ExitCode.INCORRECT_USAGE,
        stderr_contains="reserved",
    )


def test_upgrade_keeps_inline_environment(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = (
        workspace_header(multiple_versions_channel_1)
        + """
    [dependencies]
    package = "==0.1.0"

    [environments.dev.dependencies]
    package2 = "==0.1.0"
    """
    )
    manifest.write_text(toml)

    verify_cli_command([pixi, "upgrade", "--manifest-path", manifest])

    parsed = tomllib.loads(manifest.read_text())
    # The default dependency is upgraded ...
    assert parsed["dependencies"]["package"] != "==0.1.0"
    # ... and so is the inline environment dependency, in place. No feature
    # table may be written for the synthesized environment feature.
    assert parsed["environments"]["dev"]["dependencies"]["package2"] != "==0.1.0"
    assert "feature" not in parsed

    # Upgrading the reserved feature directly is rejected.
    verify_cli_command(
        [pixi, "upgrade", "--manifest-path", manifest, "--feature", "env:dev"],
        ExitCode.INCORRECT_USAGE,
        stderr_contains="reserved",
    )


def test_workspace_environment_list_shows_inline(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = (
        workspace_header(dummy_channel_1)
        + """
    [environments.dev.dependencies]
    dummy-a = "*"
    """
    )
    manifest.write_text(toml)

    # The synthesized feature is rendered with its env: prefix so it cannot be
    # mistaken for a regular feature.
    verify_cli_command(
        [pixi, "workspace", "environment", "list", "--manifest-path", manifest],
        stdout_contains=["dev", "env:dev"],
        strip_ansi=True,
    )


def test_workspace_environment_remove_inline(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = (
        workspace_header(dummy_channel_1)
        + """
    [environments.dev.dependencies]
    dummy-a = "*"
    """
    )
    manifest.write_text(toml)

    verify_cli_command(
        [pixi, "workspace", "environment", "remove", "--manifest-path", manifest, "dev"],
    )

    # The environment is gone and the manifest is still valid.
    output = verify_cli_command(
        [pixi, "workspace", "environment", "list", "--manifest-path", manifest],
    )
    assert "dev" not in output.stdout
    verify_cli_command([pixi, "install", "--manifest-path", manifest])


def test_workspace_environment_add_existing_inline(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = (
        workspace_header(dummy_channel_1)
        + """
    [environments.dev.dependencies]
    dummy-a = "*"
    """
    )
    manifest.write_text(toml)

    # Without --force the existing environment must not be clobbered.
    verify_cli_command(
        [pixi, "workspace", "environment", "add", "--manifest-path", manifest, "dev"],
        ExitCode.FAILURE,
        stderr_contains="already exists",
    )

    # With --force the environment is replaced; the manifest must stay valid.
    verify_cli_command(
        [pixi, "workspace", "environment", "add", "--manifest-path", manifest, "dev", "--force"],
    )
    verify_cli_command([pixi, "install", "--manifest-path", manifest, "--environment", "dev"])


def test_info_shows_env_feature(pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = (
        workspace_header(dummy_channel_1)
        + """
    [environments.dev.dependencies]
    dummy-a = "*"
    """
    )
    manifest.write_text(toml)

    info = verify_cli_command([pixi, "info", "--json", "--manifest-path", manifest])
    data = json.loads(info.stdout)
    dev = next(env for env in data["environments_info"] if env["name"] == "dev")
    assert "env:dev" in dev["features"]


def test_lockfile_contains_inline_env(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = (
        workspace_header(dummy_channel_1)
        + """
    [environments.dev.dependencies]
    dummy-a = "*"
    """
    )
    manifest.write_text(toml)

    verify_cli_command([pixi, "lock", "--manifest-path", manifest])
    lockfile = tmp_pixi_workspace.joinpath("pixi.lock").read_text()
    assert "dummy-a" in lockfile

    # A second lock is a no-op.
    verify_cli_command(
        [pixi, "lock", "--manifest-path", manifest],
        stderr_excludes="Updated lockfile",
    )


def test_feature_and_environment_same_name(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = (
        workspace_header(dummy_channel_1)
        + """
    [feature.dev.dependencies]
    dummy-b = "*"

    [environments.dev]
    features = ["dev"]
    dependencies = { dummy-a = "*" }
    """
    )
    manifest.write_text(toml)

    # The named feature `dev` and the implicit feature `env:dev` coexist.
    verify_cli_command(
        [pixi, "list", "--manifest-path", manifest, "--environment", "dev"],
        stdout_contains=["dummy-a", "dummy-b"],
    )


def test_add_default_dependency_preserves_inline_env(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = (
        workspace_header(dummy_channel_1)
        + """
    [environments.dev.dependencies]
    dummy-a = "*"
    """
    )
    manifest.write_text(toml)

    verify_cli_command([pixi, "add", "--manifest-path", manifest, "dummy-b"])

    parsed = tomllib.loads(manifest.read_text())
    assert parsed["environments"]["dev"]["dependencies"]["dummy-a"] == "*"
    verify_cli_command(
        [pixi, "list", "--manifest-path", manifest, "--environment", "dev"],
        stdout_contains=["dummy-a", "dummy-b"],
    )


def test_ambiguous_inline_task(pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = (
        workspace_header(dummy_channel_1)
        + """
    [environments.dev.tasks]
    greet = "echo hello-from-dev"

    [environments.test.tasks]
    greet = "echo hello-from-test"
    """
    )
    manifest.write_text(toml)

    # Running without an environment cannot disambiguate; with an explicit
    # environment it works.
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "greet"],
        ExitCode.FAILURE,
    )
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "--environment", "test", "greet"],
        stdout_contains="hello-from-test",
    )


def test_tree_inline_env(pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = (
        workspace_header(dummy_channel_1)
        + """
    [environments.dev.dependencies]
    dummy-a = "*"
    """
    )
    manifest.write_text(toml)

    verify_cli_command(
        [pixi, "tree", "--manifest-path", manifest, "--environment", "dev"],
        stdout_contains="dummy-a",
    )


def test_export_conda_environment_inline(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = (
        workspace_header(dummy_channel_1)
        + """
    [environments.dev.dependencies]
    dummy-a = "*"
    """
    )
    manifest.write_text(toml)

    verify_cli_command(
        [
            pixi,
            "workspace",
            "export",
            "conda-environment",
            "--manifest-path",
            manifest,
            "--environment",
            "dev",
        ],
        stdout_contains="dummy-a",
    )


def test_workspace_environment_add_referencing_env_feature_rejected(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = (
        workspace_header(dummy_channel_1)
        + """
    [environments.dev.dependencies]
    dummy-a = "*"
    """
    )
    manifest.write_text(toml)

    # Adding an environment that references the reserved feature must fail
    # instead of writing a manifest that no longer parses.
    verify_cli_command(
        [
            pixi,
            "workspace",
            "environment",
            "add",
            "--manifest-path",
            manifest,
            "other",
            "--feature",
            "env:dev",
        ],
        ExitCode.FAILURE,
        stderr_contains="cannot be referenced",
    )
    verify_cli_command([pixi, "install", "--manifest-path", manifest, "--environment", "dev"])

    # A near-miss on the bare environment name must not suggest the
    # synthesized feature, which cannot be referenced.
    verify_cli_command(
        [
            pixi,
            "workspace",
            "environment",
            "add",
            "--manifest-path",
            manifest,
            "other",
            "--feature",
            "dev",
        ],
        ExitCode.FAILURE,
        stderr_contains="not defined",
        stderr_excludes="env:dev",
    )


def test_inline_solve_strategy(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = (
        workspace_header(multiple_versions_channel_1)
        + """
    [environments.dev]
    dependencies = { package = "*" }
    solve-strategy = "lowest"
    """
    )
    manifest.write_text(toml)

    verify_cli_command(
        [pixi, "list", "--manifest-path", manifest, "--environment", "dev"],
        stdout_contains="0.1.0",
        stdout_excludes="0.2.0",
    )


def test_remove_suggestion_is_actionable(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = (
        workspace_header(dummy_channel_1)
        + """
    [dependencies]
    dummy-b = "*"

    [environments.dev.dependencies]
    dummy-a = "*"
    """
    )
    manifest.write_text(toml)

    # The dependency only exists inline on the environment. The error must not
    # suggest `--feature env:dev`, because that flag value is rejected as
    # reserved by the CLI.
    verify_cli_command(
        [pixi, "remove", "--manifest-path", manifest, "dummy-a"],
        ExitCode.FAILURE,
        stderr_contains="[environments.dev.dependencies]",
        stderr_excludes="--feature env:dev",
    )


@pytest.mark.skipif(sys.platform == "win32", reason="drives a confirmation prompt through a pty")
@pytest.mark.filterwarnings("ignore::DeprecationWarning")
def test_remove_feature_keeps_inline_content(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    import pty

    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = (
        workspace_header(dummy_channel_1)
        + """
    [feature.shared.dependencies]
    dummy-b = "*"

    [environments.dev]
    features = ["shared"]
    dependencies = { dummy-a = "*" }
    """
    )
    manifest.write_text(toml)

    # `pixi workspace feature remove` asks for confirmation when the feature
    # is used by an environment, so answer "y" through a pty.
    pid, fd = pty.fork()
    if pid == 0:
        os.execv(
            str(pixi),
            [
                str(pixi),
                "workspace",
                "feature",
                "remove",
                "--manifest-path",
                str(manifest),
                "shared",
            ],
        )
    time.sleep(2)
    os.write(fd, b"y\n")
    while True:
        try:
            if not os.read(fd, 4096):
                break
        except OSError:
            break
    _, status = os.waitpid(pid, 0)
    assert os.waitstatus_to_exitcode(status) == 0

    # The inline dependencies must survive the rewrite of the environment
    # entry and the manifest must still be valid.
    verify_cli_command(
        [pixi, "list", "--manifest-path", manifest, "--environment", "dev"],
        stdout_contains="dummy-a",
        stdout_excludes="dummy-b",
    )


def test_import_into_inline_environment_keeps_content(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = (
        workspace_header(dummy_channel_1)
        + """
    [environments.dev.dependencies]
    dummy-a = "*"
    """
    )
    manifest.write_text(toml)

    environment_yml = tmp_pixi_workspace.joinpath("environment.yml")
    environment_yml.write_text(
        f"""
        name: imported
        channels:
          - {dummy_channel_1}
        dependencies:
          - dummy-b
        """
    )

    verify_cli_command(
        [
            pixi,
            "import",
            "--manifest-path",
            manifest,
            "--environment",
            "dev",
            environment_yml,
        ],
    )

    # The imported feature is added to the environment while the inline
    # dependencies survive.
    parsed = tomllib.loads(manifest.read_text())
    assert parsed["environments"]["dev"]["dependencies"]["dummy-a"] == "*"
    verify_cli_command(
        [pixi, "list", "--manifest-path", manifest, "--environment", "dev"],
        stdout_contains=["dummy-a", "dummy-b"],
    )


def test_inline_channel_priority_and_solve_strategy(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str, dummy_channel_2: str
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = (
        workspace_header(dummy_channel_1)
        + f"""
    [environments.dev]
    channels = ["{dummy_channel_2}"]
    channel-priority = "disabled"
    dependencies = {{ dummy-b = "*" }}
    """
    )
    manifest.write_text(toml)

    verify_cli_command(
        [pixi, "list", "--manifest-path", manifest, "--environment", "dev"],
        stdout_contains="dummy-b",
    )
