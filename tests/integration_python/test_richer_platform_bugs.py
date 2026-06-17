"""Known bugs in the richer-platform / system-requirement model.

Every test here asserts the *intended* behaviour and is marked
``xfail(strict=True)`` because pixi currently gets it wrong. When a bug is
fixed its test starts passing, the strict xfail turns that into a failure, and
the marker (and this note) should be removed.

All tests stay network-free: they use the in-repo ``virtual_packages`` channel
(its ``cuda`` package depends on ``__cuda >=12``) and gate themselves on the
host platform where the requirement only makes sense.
"""

from __future__ import annotations

from pathlib import Path

import pytest

from .common import CURRENT_PLATFORM, ExitCode, verify_cli_command

# The virtual_packages channel only ships the cuda package for these subdirs.
CUDA_CHANNEL_SUBDIRS = {"linux-64", "win-64"}

requires_cuda_channel = pytest.mark.skipif(
    CURRENT_PLATFORM not in CUDA_CHANNEL_SUBDIRS,
    reason="virtual_packages channel ships the cuda package only for linux-64 and win-64",
)
linux_only = pytest.mark.skipif(
    not CURRENT_PLATFORM.startswith("linux"),
    reason="a linux system requirement only gates installs on linux hosts",
)


def _write(manifest: Path, body: str) -> Path:
    manifest.write_text(body)
    return manifest


@requires_cuda_channel
def test_cuda_requirement_does_not_block_install(
    pixi: Path, tmp_pixi_workspace: Path, virtual_packages_channel: str
) -> None:
    """A cuda system requirement must not block installing an environment whose
    dependencies do not need cuda."""
    manifest = _write(
        tmp_pixi_workspace / "pixi.toml",
        f"""
[workspace]
name = "cuda-block"
channels = ["{virtual_packages_channel}"]
platforms = ["{CURRENT_PLATFORM}"]

[system-requirements]
cuda = "42"

[dependencies]
no-deps = "*"
""",
    )
    verify_cli_command([pixi, "install", "--manifest-path", manifest], ExitCode.SUCCESS)


@requires_cuda_channel
def test_cuda_override_at_package_floor_installs(
    pixi: Path, tmp_pixi_workspace: Path, virtual_packages_channel: str
) -> None:
    """At the package floor (``__cuda >=12``) the environment must install,
    regardless of the higher declared ``cuda = "42"``."""
    manifest = _write(
        tmp_pixi_workspace / "pixi.toml",
        f"""
[workspace]
name = "cuda-floor"
channels = ["{virtual_packages_channel}"]
platforms = ["{CURRENT_PLATFORM}"]

[system-requirements]
cuda = "42"

[dependencies]
cuda = "*"
""",
    )
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
        ExitCode.SUCCESS,
        env={"CONDA_OVERRIDE_CUDA": "12"},
    )


@linux_only
@pytest.mark.xfail(
    strict=True,
    reason="an empty environment has nothing to install, yet an unsatisfiable "
    "linux requirement blocks running its task",
)
def test_task_runs_in_empty_environment(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """A task in an empty environment must always run, even when the declared
    linux requirement exceeds the host kernel."""
    manifest = _write(
        tmp_pixi_workspace / "pixi.toml",
        f"""
[workspace]
name = "empty-task"
channels = []
platforms = ["{CURRENT_PLATFORM}"]

[system-requirements]
linux = "8.0"

[tasks]
task1 = "echo task1"
""",
    )
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "task1"],
        ExitCode.SUCCESS,
        stdout_contains="task1",
    )


@linux_only
def test_task_runs_with_kernel_agnostic_dependency(
    pixi: Path, tmp_pixi_workspace: Path, virtual_packages_channel: str
) -> None:
    """A task must run when its dependency is kernel-agnostic, even with a
    declared linux requirement the host cannot satisfy."""
    manifest = _write(
        tmp_pixi_workspace / "pixi.toml",
        f"""
[workspace]
name = "dep-task"
channels = ["{virtual_packages_channel}"]
platforms = ["{CURRENT_PLATFORM}"]

[system-requirements]
linux = "8.0"

[dependencies]
no-deps = "*"

[tasks]
task1 = "echo task1"
""",
    )
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "task1"],
        ExitCode.SUCCESS,
        stdout_contains="task1",
    )
