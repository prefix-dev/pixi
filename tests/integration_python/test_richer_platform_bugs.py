"""Regression tests for the richer-platform / system-requirement model.

Each test asserts the intended behaviour for a bug that pixi used to get
wrong; they guard against regressions now that those bugs are fixed.

All tests stay network-free: they use the in-repo ``virtual_packages`` channel
(its ``cuda`` package depends on ``__cuda >=12``) or ``dummy_channel_1``, and
gate themselves on the host platform where the requirement only makes sense.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

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


# A resolved platform no machine satisfies. `classify_run_platform` answers on
# the resolved platform first and never consults the recorded minimum when that
# one matches, so tests about a requirement must make it unsatisfiable first.
_UNSATISFIABLE_RESOLVED: dict[str, Any] = {
    "subdir": CURRENT_PLATFORM,
    "virtual_packages": ["__cuda=99999"],
}


def _bare_manifest(workspace: Path, channel: str, *, deps: str = 'no-deps = "*"') -> Path:
    """A workspace on a bare subdir platform, so the declared-platform check
    always passes and the ``conda-meta/pixi`` marker check is what decides."""
    return _write(
        workspace / "pixi.toml",
        f"""
[workspace]
name = "marker-check"
channels = ["{channel}"]
platforms = ["{CURRENT_PLATFORM}"]

[dependencies]
{deps}

[tasks]
hello = "echo TASK-RAN"
""",
    )


def _marker(workspace: Path, environment: str = "default") -> Path:
    return workspace / ".pixi" / "envs" / environment / "conda-meta" / "pixi"


def _read_marker(workspace: Path, environment: str = "default") -> dict[str, Any]:
    return json.loads(_marker(workspace, environment).read_text())


def _patch_marker(workspace: Path, **fields: Any) -> None:
    """Overwrite marker fields in place, to reach states a solve cannot produce."""
    path = _marker(workspace)
    data = json.loads(path.read_text())
    data.update(fields)
    path.write_text(json.dumps(data))


def _install(pixi: Path, manifest: Path, **kwargs: Any) -> None:
    verify_cli_command([pixi, "install", "--manifest-path", manifest], ExitCode.SUCCESS, **kwargs)


def _run(pixi: Path, manifest: Path, expected: ExitCode, **kwargs: Any) -> None:
    verify_cli_command([pixi, "run", "--manifest-path", manifest, "hello"], expected, **kwargs)


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
        stderr_contains=[
            # The declared platform, reported as the capability it declares...
            "__cuda >= 42",
            # ...and the package's own floor, reported as the spec it depends on.
            "__cuda >=12",
        ],
    )


@linux_only
@requires_cuda_channel
def test_run_honours_platform_override_for_the_marker_check(
    pixi: Path, tmp_pixi_workspace: Path, virtual_packages_channel: str
) -> None:
    """``PIXI_OVERRIDE_PLATFORM`` must skip the ``conda-meta/pixi`` marker check.

    The override moves the subdir pixi pretends to be on but leaves the detected
    virtual packages alone, so the check used to compare a pretended ``win-64``
    against a marker recorded for ``linux-64`` and report every requirement as
    unmet -- naming ``__cuda``, which the machine does provide. Every sibling
    platform check already bails on the override.
    """
    manifest = _write(
        tmp_pixi_workspace / "pixi.toml",
        f"""
[workspace]
name = "override-run"
channels = ["{virtual_packages_channel}"]
platforms = [{{ platform = "linux-64", cuda = "13" }}]

[dependencies]
cuda = "*"

[tasks]
hello = "echo marker-check-skipped"
""",
    )
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
        ExitCode.SUCCESS,
        env={"CONDA_OVERRIDE_CUDA": "13"},
    )

    # Pretending to be win-64 must not resurrect the linux-64 marker.
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "hello"],
        ExitCode.SUCCESS,
        env={"PIXI_OVERRIDE_PLATFORM": "win-64", "CONDA_OVERRIDE_CUDA": "13"},
        stdout_contains="marker-check-skipped",
    )

    # Without the override the check still bites: a host below the package floor
    # cannot run what was installed.
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "hello"],
        ExitCode.FAILURE,
        env={"CONDA_OVERRIDE_CUDA": "1"},
        stderr_contains="__cuda >=12",
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


def test_unreadable_requirement_costs_only_that_requirement(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    """A requirement pixi cannot parse must not disable the whole check.

    Rejecting the platform means ``read_environment_file`` deletes the marker,
    which silently reinstalls the prefix and skips the very check the marker
    exists for -- so an unreadable entry would turn a refusal into a pass.
    """
    manifest = _bare_manifest(tmp_pixi_workspace, dummy_channel_1, deps='dummy-a = "*"')
    _install(pixi, manifest)
    _patch_marker(
        tmp_pixi_workspace,
        resolved_platform=_UNSATISFIABLE_RESOLVED,
        minimum_supported_platform={
            "subdir": CURRENT_PLATFORM,
            # Too large for a conda version component, so it cannot be read.
            "requirements": ["__cuda >=99999999999999999999", "__cuda >=12"],
        },
    )
    _run(pixi, manifest, ExitCode.FAILURE, stderr_contains="__cuda >=12")
    # The marker survives, so the readable requirement is still enforced next time.
    assert "requirements" in _read_marker(tmp_pixi_workspace)["minimum_supported_platform"]


def test_unreadable_requirement_alone_does_not_discard_the_marker(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    """The rest of the marker -- lock hash, resolved platform, fingerprints --
    must outlive an entry pixi cannot read."""
    manifest = _bare_manifest(tmp_pixi_workspace, dummy_channel_1, deps='dummy-a = "*"')
    _install(pixi, manifest)
    _patch_marker(
        tmp_pixi_workspace,
        minimum_supported_platform={
            "subdir": CURRENT_PLATFORM,
            "requirements": [f"__cuda >={'9' * 200}"],
        },
    )
    before = _read_marker(tmp_pixi_workspace)["environment_lock_file_hash"]
    _run(pixi, manifest, ExitCode.SUCCESS, stdout_contains="TASK-RAN")
    assert _read_marker(tmp_pixi_workspace)["environment_lock_file_hash"] == before
