from __future__ import annotations

import json
from pathlib import Path

import pytest

from .common import CURRENT_PLATFORM, ExitCode, verify_cli_command
from .test_virtual_package_detectors import DetectorChannel

pytestmark = pytest.mark.skipif(
    CURRENT_PLATFORM != "linux-64",
    reason="uses Linux shell activation and deterministic CUDA overrides",
)


@pytest.fixture
def bounded_platform_workspace(tmp_pixi_workspace: Path) -> Path:
    channel = DetectorChannel(tmp_pixi_workspace / "channel")
    for name, requirements in (
        ("gpu", ["__cuda >=12,<13"]),
        ("cpu", []),
    ):
        channel.package(
            f"{name}-tool",
            requirements,
            {"bin/selection-tool": f"#!/bin/sh\necho PACKAGE-{name}\n".encode()},
        )
        (tmp_pixi_workspace / f"{name}.sh").write_text(f"export SCRIPT_SELECTION={name}\n")
    (tmp_pixi_workspace / "pixi.toml").write_text(f"""
[workspace]
name = "selection-review"
channels = ["{channel.url}"]
platforms = [
    {{ name = "gpu", platform = "linux-64", cuda = "12" }},
    {{ name = "cpu", platform = "linux-64" }},
]

[tasks]
selected = "echo ACTIVE-$SELECTION-$SCRIPT_SELECTION"

[target.gpu.dependencies]
gpu-tool = "*"

[target.cpu.dependencies]
cpu-tool = "*"

[target.gpu.activation]
scripts = ["gpu.sh"]

[target.gpu.activation.env]
SELECTION = "gpu"

[target.cpu.activation]
scripts = ["cpu.sh"]

[target.cpu.activation.env]
SELECTION = "cpu"

[target.gpu.tasks]
gpu-only = "echo GPU-TASK"

[target.cpu.tasks]
cpu-only = "echo CPU-TASK"
""")
    return tmp_pixi_workspace


def _marker(workspace: Path, environment: str = "default") -> Path:
    return workspace / ".pixi" / "envs" / environment / "conda-meta" / "pixi"


@pytest.mark.parametrize("json_output", [False, True])
def test_shell_hook_uses_selected_activation_script_without_installing(
    pixi: Path, bounded_platform_workspace: Path, json_output: bool
) -> None:
    workspace = bounded_platform_workspace
    verify_cli_command([pixi, "lock"], cwd=workspace)
    lock_before = (workspace / "pixi.lock").read_bytes()
    args = ["--json"] if json_output else ["--shell", "bash"]
    output = verify_cli_command(
        [pixi, "shell-hook", "--frozen", "--no-install", *args],
        cwd=workspace,
        env={"CONDA_OVERRIDE_CUDA": "14"},
    )
    if json_output:
        hook = json.loads(output.stdout)
        assert hook["environment_variables"]["SELECTION"] == "cpu"
        assert hook["environment_variables"]["SCRIPT_SELECTION"] == "cpu"
        assert [Path(script).name for script in hook["activation_scripts"]] == ["cpu.sh"]
    else:
        verify_cli_command(
            ["bash", "--noprofile", "--norc"],
            stdin=output.stdout + '\nprintf "HOOK-%s-%s\\n" "$SELECTION" "$SCRIPT_SELECTION"\n',
            cwd=workspace,
            stdout_contains="HOOK-cpu-cpu",
        )
    assert (workspace / "pixi.lock").read_bytes() == lock_before
    assert not _marker(workspace).exists()


def test_reinstall_changes_incompatible_cached_prefix_and_its_activation(
    pixi: Path, bounded_platform_workspace: Path
) -> None:
    workspace = bounded_platform_workspace
    verify_cli_command(
        [pixi, "run", "--platform", "gpu", "selection-tool"],
        cwd=workspace,
        env={"CONDA_OVERRIDE_CUDA": "12"},
        stdout_contains="PACKAGE-gpu",
    )
    marker_before = _marker(workspace).read_bytes()
    host = {"CONDA_OVERRIDE_CUDA": "14"}
    for options in (["--no-install"], []):
        verify_cli_command(
            [pixi, "run", "--frozen", *options, "selection-tool"],
            ExitCode.FAILURE,
            cwd=workspace,
            env=host,
            stdout_excludes=["PACKAGE-gpu", "PACKAGE-cpu"],
            stderr_contains="__cuda",
        )
        assert _marker(workspace).read_bytes() == marker_before
    verify_cli_command([pixi, "install", "--frozen"], cwd=workspace, env=host)
    assert json.loads(_marker(workspace).read_text())["resolved_platform"]["name"] == "cpu"
    verify_cli_command(
        [pixi, "run", "--frozen", "selection-tool"],
        cwd=workspace,
        env=host,
        stdout_contains="PACKAGE-cpu",
        stdout_excludes="PACKAGE-gpu",
    )
    verify_cli_command(
        [pixi, "run", "--frozen", "selected"],
        cwd=workspace,
        env=host,
        stdout_contains="ACTIVE-cpu-cpu",
    )


def test_removing_last_dependencies_clears_existing_prefix(
    pixi: Path, bounded_platform_workspace: Path
) -> None:
    workspace = bounded_platform_workspace
    host = {"CONDA_OVERRIDE_CUDA": "13"}
    verify_cli_command(
        [pixi, "run", "selection-tool"],
        cwd=workspace,
        env=host,
        stdout_contains="PACKAGE-cpu",
    )
    manifest = workspace / "pixi.toml"
    manifest.write_text(
        manifest.read_text().replace('gpu-tool = "*"', "").replace('cpu-tool = "*"', "")
    )
    verify_cli_command(
        [pixi, "run", "selection-tool"],
        ExitCode.COMMAND_NOT_FOUND,
        cwd=workspace,
        env=host,
        stdout_excludes="PACKAGE-cpu",
    )
    assert not (workspace / ".pixi" / "envs" / "default" / "bin" / "selection-tool").exists()


@pytest.mark.parametrize("no_install", [False, True])
def test_task_search_can_leave_an_incompatible_default_environment(
    pixi: Path, tmp_pixi_workspace: Path, no_install: bool
) -> None:
    channel = DetectorChannel(tmp_pixi_workspace / "channel")
    channel.package("gpu-tool", ["__cuda >=12,<13"])
    channel.package("cpu-tool", [])
    (tmp_pixi_workspace / "pixi.toml").write_text(f"""
[workspace]
name = "task-search-review"
channels = ["{channel.url}"]
platforms = [
    {{ name = "gpu", platform = "linux-64", cuda = "12" }},
    {{ name = "cpu", platform = "linux-64" }},
]

[tasks]
hello = "echo WRONG-DEFAULT"

[feature.gpu]
platforms = ["gpu"]

[feature.gpu.dependencies]
gpu-tool = "*"

[feature.cpu]
platforms = ["cpu"]

[feature.cpu.dependencies]
cpu-tool = "*"

[feature.cpu.activation.env]
SELECTION = "cpu"

[feature.cpu.target.cpu.tasks]
hello = "echo WORKER-$SELECTION"

[environments]
default = ["gpu"]
worker = {{ features = ["cpu"], no-default-feature = true }}
""")
    verify_cli_command([pixi, "lock"], cwd=tmp_pixi_workspace)
    options = ["--no-install"] if no_install else []
    host = {"CONDA_OVERRIDE_CUDA": "14"}
    verify_cli_command(
        [pixi, "run", "--frozen", *options, "hello"],
        cwd=tmp_pixi_workspace,
        env=host,
        stdout_contains="WORKER-cpu",
        stdout_excludes="WRONG-DEFAULT",
    )
    assert not _marker(tmp_pixi_workspace).exists()
    assert _marker(tmp_pixi_workspace, "worker").exists() == (not no_install)
    verify_cli_command(
        [pixi, "run", "--frozen", *options, "--environment", "default", "hello"],
        ExitCode.FAILURE,
        cwd=tmp_pixi_workspace,
        env=host,
        stdout_excludes=["WORKER-cpu", "WRONG-DEFAULT"],
    )


def test_explicit_platform_controls_task_listing_without_creating_lock(
    pixi: Path, bounded_platform_workspace: Path
) -> None:
    workspace = bounded_platform_workspace
    output = verify_cli_command(
        [pixi, "run", "--platform", "cpu"],
        cwd=workspace,
        env={"CONDA_OVERRIDE_CUDA": "12"},
        strip_ansi=True,
    )
    listing = output.stdout + output.stderr
    assert "cpu-only" in listing
    assert "gpu-only" not in listing
    assert not (workspace / "pixi.lock").exists()
    assert not _marker(workspace).exists()
