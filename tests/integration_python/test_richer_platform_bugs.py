"""Regression tests for the richer-platform / system-requirement model.

Each test asserts the intended behaviour for a bug that pixi used to get
wrong; they guard against regressions now that those bugs are fixed.

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


@requires_cuda_channel
def test_cuda_override_below_package_floor_is_refused(
    pixi: Path, tmp_pixi_workspace: Path, virtual_packages_channel: str
) -> None:
    """Guard rail (passes today): below the package's ``__cuda >=12`` floor the
    install must fail."""
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
        ExitCode.FAILURE,
        env={"CONDA_OVERRIDE_CUDA": "10"},
        stderr_contains="__cuda >= 12",
    )


@requires_cuda_channel
def test_run_without_environment_flag_does_not_leak_base_platform(
    pixi: Path, tmp_pixi_workspace: Path, virtual_packages_channel: str
) -> None:
    """``pixi run <task>`` (no ``-e``) must install the task's own environment
    for a platform that environment declares, not the base environment's.

    Repro of the GPU-CI failure: once the base ``default`` environment is
    installed, its ``conda-meta/pixi`` marker records the bare-subdir resolved
    platform. Running a task that lives in a *different* environment then pinned
    that bare subdir as the global target platform for every prefix install. The
    ``gpu`` environment only declares the rich ``<subdir>-cuda-13`` platform, so
    the bare subdir is not one of its platforms and the install aborted with
    "no platform supported by it matches the current system" -- even though the
    (cuda-capable) host can run it. Running the same task with ``-e gpu`` always
    worked, because that resolves the platform for ``gpu`` directly.
    """
    manifest = _write(
        tmp_pixi_workspace / "pixi.toml",
        f"""
[workspace]
name = "target-platform-leak"
channels = ["{virtual_packages_channel}"]
platforms = ["{CURRENT_PLATFORM}"]

[dependencies]
no-deps = "*"

[feature.gpu.system-requirements]
cuda = "13"

[feature.gpu.dependencies]
cuda = "*"

[feature.gpu.tasks]
gpu-task = "echo gpu_task_ran"

[environments]
gpu = {{ features = ["gpu"], no-default-feature = true }}
""",
    )

    # Install the base `default` environment first so its marker records the
    # bare-subdir resolved platform -- the state the leak needs to trigger.
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest, "--environment", "default"],
        ExitCode.SUCCESS,
        env={"CONDA_OVERRIDE_CUDA": "13"},
    )

    # Without `-e`, running the gpu task must still install and run it via the
    # gpu environment's own (rich) platform, not the leaked bare subdir.
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "gpu-task"],
        ExitCode.SUCCESS,
        env={"CONDA_OVERRIDE_CUDA": "13"},
        stdout_contains="gpu_task_ran",
    )


@linux_only
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


def test_sysreq_platform_restriction_lock_check_converges(
    pixi: Path, tmp_pixi_workspace: Path, virtual_packages_channel: str
) -> None:
    """A ``[system-requirements]`` table combined with a feature that restricts
    platforms must produce a lock file that passes ``pixi lock --check``.

    Repro of the never-converging lock: the environment combining both
    features was composed from the subdir default virtual packages instead of
    the declared baseline, identity-equal to the bare subdir platform, so the
    lock-file name restore was ambiguous and every check re-solved to the
    same lock. No host gating: locking solves the declared platforms, and
    ``linux-64`` is declared literally so the libc requirement applies.
    """
    manifest = _write(
        tmp_pixi_workspace / "pixi.toml",
        f"""
[workspace]
name = "sysreq-restriction"
channels = ["{virtual_packages_channel}"]
platforms = ["linux-64"]

[system-requirements]
libc = "2.17"

[dependencies]
no-deps = "*"

[feature.x86-only]
platforms = ["linux-64"]

[environments]
restricted = {{ features = ["x86-only"] }}
""",
    )
    verify_cli_command([pixi, "lock", "--manifest-path", manifest], ExitCode.SUCCESS)
    verify_cli_command(
        [pixi, "lock", "--check", "--dry-run", "--manifest-path", manifest],
        ExitCode.SUCCESS,
    )
