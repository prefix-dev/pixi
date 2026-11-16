"""Platform selection across installation, inspection, scripts, and task dependencies."""

from __future__ import annotations

import errno
import json
import os
import select
import signal
import time
from pathlib import Path

import pytest
import yaml
from inline_snapshot import snapshot

from .common import CURRENT_PLATFORM, ExitCode, verify_cli_command
from .test_virtual_package_detectors import DetectorChannel

pytestmark = pytest.mark.skipif(
    CURRENT_PLATFORM != "linux-64",
    reason="uses deterministic Linux CUDA overrides and shell expansion",
)


def _channel(root: Path) -> DetectorChannel:
    channel = DetectorChannel(root / "channel")
    channel.package("cpu-package", [])
    channel.package("gpu-package", ["__cuda >=12"])
    return channel


def _marker(root: Path, environment: str = "default") -> Path:
    return root / ".pixi" / "envs" / environment / "conda-meta" / "pixi"


def _tighten_gpu_requirement(lock: Path) -> None:
    contents = lock.read_text()
    assert "__cuda >=12" in contents
    lock.write_text(contents.replace("__cuda >=12", "__cuda >=12.4"))


@pytest.mark.parametrize("include_cpu", [True, False])
def test_install_all_without_lock_skips_gpu_only_environment(
    pixi: Path, tmp_pixi_workspace: Path, include_cpu: bool
) -> None:
    channel = _channel(tmp_pixi_workspace)
    manifest = tmp_pixi_workspace / "pixi.toml"
    cpu_platform = '{ name = "cpu", platform = "linux-64" },' if include_cpu else ""
    manifest.write_text(f"""
[workspace]
name = "fresh-all"
channels = ["{channel.url}"]
platforms = [
    {{ name = "gpu", platform = "linux-64", cuda = "12" }},
    {cpu_platform}
]

[dependencies]
{"cpu-package" if include_cpu else "gpu-package"} = "*"

[feature.accelerated]
platforms = ["gpu"]

[feature.accelerated.dependencies]
gpu-package = "*"

[environments]
accelerated = {{ features = ["accelerated"], no-default-feature = true }}
""")
    verify_cli_command(
        [pixi, "install", "--all", "--manifest-path", manifest],
        env={"CONDA_OVERRIDE_CUDA": "11.8"},
    )
    assert _marker(tmp_pixi_workspace).exists() == include_cpu
    assert not _marker(tmp_pixi_workspace, "accelerated").exists()


def test_install_all_includes_environment_added_since_last_lock(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    channel = _channel(tmp_pixi_workspace)
    manifest = tmp_pixi_workspace / "pixi.toml"
    contents = f"""
[workspace]
name = "new-environment"
channels = ["{channel.url}"]
platforms = ["linux-64"]

[dependencies]
cpu-package = "*"
"""
    manifest.write_text(contents)
    verify_cli_command([pixi, "lock", "--manifest-path", manifest])
    manifest.write_text(contents + "\n[environments]\nnewly-added = []\n")
    verify_cli_command([pixi, "install", "--all", "--manifest-path", manifest])
    assert _marker(tmp_pixi_workspace, "newly-added").exists(), (
        "a missing environment in the stale lock must not exclude a newly declared environment"
    )


def test_install_all_rechecks_requirements_after_manifest_edit(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    channel = _channel(tmp_pixi_workspace)
    manifest = tmp_pixi_workspace / "pixi.toml"
    contents = f"""
[workspace]
name = "changed-requirement"
channels = ["{channel.url}"]
platforms = [{{ name = "gpu", platform = "linux-64", cuda = "12" }}]

[dependencies]
gpu-package = "*"
"""
    manifest.write_text(contents)
    verify_cli_command([pixi, "lock", "--manifest-path", manifest])
    manifest.write_text(contents.replace('gpu-package = "*"', 'cpu-package = "*"'))
    verify_cli_command(
        [pixi, "install", "--all", "--manifest-path", manifest],
        env={"CONDA_OVERRIDE_CUDA": "11.8"},
    )
    assert _marker(tmp_pixi_workspace).exists(), (
        "obsolete locked GPU requirements must not exclude the now CPU-only environment"
    )


@pytest.mark.parametrize("no_install", [True, False])
def test_remove_succeeds_on_host_that_cannot_run_environment(
    pixi: Path, tmp_pixi_workspace: Path, no_install: bool
) -> None:
    """A manifest edit must not fail after already rewriting the manifest and
    lock file: a lock-only edit selects no runnable platform, and otherwise the
    install is skipped like `pixi add` does."""
    channel = _channel(tmp_pixi_workspace)
    manifest = tmp_pixi_workspace / "pixi.toml"
    manifest.write_text(f"""
[workspace]
name = "lock-only-remove"
channels = ["{channel.url}"]
platforms = [{{ name = "gpu", platform = "linux-64", cuda = "12" }}]

[dependencies]
cpu-package = "*"
gpu-package = "*"
""")
    verify_cli_command([pixi, "lock", "--manifest-path", manifest])
    verify_cli_command(
        [
            pixi,
            "remove",
            *(["--no-install"] if no_install else []),
            "--manifest-path",
            manifest,
            "cpu-package",
        ],
        env={"CONDA_OVERRIDE_CUDA": ""},
    )
    assert "cpu-package" not in (tmp_pixi_workspace / "pixi.lock").read_text()
    assert not _marker(tmp_pixi_workspace).exists()


def test_conda_script_activates_the_platform_chosen_from_lock(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    channel = _channel(tmp_pixi_workspace)
    script = tmp_pixi_workspace / "selection.code"
    script.write_text(f"""# /// conda-script
# channels = ["{channel.url}"]
# entrypoint = "printenv SELECTION"
#
# [tool.pixi.workspace]
# platforms = [
#     {{ name = "gpu", platform = "linux-64", cuda = "12" }},
#     {{ name = "cpu", platform = "linux-64" }},
# ]
#
# [tool.pixi.target.gpu.dependencies]
# gpu-package = "*"
#
# [tool.pixi.target.cpu.dependencies]
# cpu-package = "*"
#
# [tool.pixi.target.gpu.activation.env]
# SELECTION = "wrong-gpu"
#
# [tool.pixi.target.cpu.activation.env]
# SELECTION = "right-cpu"
# /// end-conda-script
""")
    verify_cli_command([pixi, "lock", "--script", script])
    _tighten_gpu_requirement(tmp_pixi_workspace / "selection.code.pixi.lock")
    verify_cli_command(
        [pixi, "run", "--experimental", "--script", script, "--frozen"],
        env={"CONDA_OVERRIDE_CUDA": "12.2"},
        stdout_contains="right-cpu",
        stdout_excludes="wrong-gpu",
    )


@pytest.mark.parametrize("command", ["list", "tree", "export"])
def test_read_only_consumers_select_locked_fallback_without_prefix(
    pixi: Path, tmp_pixi_workspace: Path, command: str
) -> None:
    channel = _channel(tmp_pixi_workspace)
    manifest = tmp_pixi_workspace / "pixi.toml"
    manifest.write_text(f"""
[workspace]
name = "read-only-fallback"
channels = ["{channel.url}"]
platforms = [
    {{ name = "gpu", platform = "linux-64", cuda = "12" }},
    {{ name = "cpu", platform = "linux-64" }},
]

[target.gpu.dependencies]
gpu-package = "*"

[target.cpu.dependencies]
cpu-package = "*"
""")
    verify_cli_command([pixi, "lock", "--manifest-path", manifest])
    _tighten_gpu_requirement(tmp_pixi_workspace / "pixi.lock")
    args = (
        ["workspace", "export", "conda-environment", "--from-lock-file"]
        if command == "export"
        else [command, "--frozen", "--no-install"]
    )
    verify_cli_command(
        [pixi, *args, "--manifest-path", manifest],
        env={"CONDA_OVERRIDE_CUDA": "12.2"},
        stdout_contains="cpu-package",
        stdout_excludes="gpu-package",
    )
    assert not _marker(tmp_pixi_workspace).exists()


@pytest.mark.parametrize("no_install", [False, True])
def test_task_dependency_uses_its_own_selected_platform_and_activation(
    pixi: Path, tmp_pixi_workspace: Path, no_install: bool
) -> None:
    channel = _channel(tmp_pixi_workspace)
    manifest = tmp_pixi_workspace / "pixi.toml"
    manifest.write_text(f"""
[workspace]
name = "task-platforms"
channels = ["{channel.url}"]
platforms = [
    {{ name = "gpu", platform = "linux-64", cuda = "12" }},
    {{ name = "cpu", platform = "linux-64" }},
]

[target.gpu.activation.env]
SELECTION = "gpu"

[target.cpu.activation.env]
SELECTION = "cpu"

[tasks]
root = {{ cmd = "echo ROOT-$SELECTION", depends-on = [{{ task = "child", environment = "worker" }}] }}

[feature.worker.target.gpu.dependencies]
gpu-package = "*"

[feature.worker.target.cpu.dependencies]
cpu-package = "*"

[feature.worker.tasks]
child = "echo CHILD-$SELECTION"

[feature.worker.target.gpu.tasks]
child = "echo WRONG-GPU-CHILD"

[environments]
worker = ["worker"]
""")
    verify_cli_command([pixi, "lock", "--manifest-path", manifest])
    _tighten_gpu_requirement(tmp_pixi_workspace / "pixi.lock")
    verify_cli_command(
        [
            pixi,
            "run",
            "--frozen",
            *(["--no-install"] if no_install else []),
            "--manifest-path",
            manifest,
            "root",
        ],
        env={"CONDA_OVERRIDE_CUDA": "12.2"},
        stdout_contains=["ROOT-gpu", "CHILD-cpu"],
        stdout_excludes="WRONG-GPU-CHILD",
    )
    assert not _marker(tmp_pixi_workspace).exists()
    if no_install:
        assert not _marker(tmp_pixi_workspace, "worker").exists()
        return
    worker_marker = json.loads(_marker(tmp_pixi_workspace, "worker").read_text())
    assert worker_marker["resolved_platform"]["name"] == "cpu"


def test_foreign_platform_override_skips_locked_host_validation(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    channel = _channel(tmp_pixi_workspace)
    channel.package("foreign-gpu", ["__cuda >=12,<13"])
    foreign = channel.root / "win-64"
    foreign.mkdir()
    (foreign / "repodata.json").write_text(
        json.dumps({"info": {"subdir": "win-64"}, "packages": {}, "packages.conda": {}})
    )
    manifest = tmp_pixi_workspace / "pixi.toml"
    manifest.write_text(f"""
[workspace]
channels = ["{channel.url}"]
platforms = [{{ name = "gpu", platform = "win-64", cuda = "12" }}]

[dependencies]
foreign-gpu = "*"
""")
    verify_cli_command([pixi, "lock", "--manifest-path", manifest])
    verify_cli_command(
        [pixi, "install", "--frozen", "--manifest-path", manifest],
        env={"PIXI_OVERRIDE_PLATFORM": "win-64", "CONDA_OVERRIDE_CUDA": "13"},
    )
    marker = json.loads(_marker(tmp_pixi_workspace).read_text())
    assert marker["resolved_platform"]["subdir"] == "win-64"


def _bounded_gpu_workspace(root: Path) -> None:
    """GPU target whose locked package rejects CUDA 12.5, next to a CPU target."""
    channel = _channel(root)
    channel.package("bounded-gpu-package", ["__cuda >=12,<12.2"])
    (root / "pixi.toml").write_text(f"""
[workspace]
name = "bounded-gpu"
channels = ["{channel.url}"]
platforms = [
    {{ name = "gpu", platform = "linux-64", cuda = "12" }},
    {{ name = "cpu", platform = "linux-64" }},
]

[target.gpu.dependencies]
bounded-gpu-package = "*"

[target.gpu.activation.env]
SELECTION = "gpu"

[target.gpu.tasks]
gpu-only = "echo gpu-only"

[target.cpu.dependencies]
cpu-package = "*"

[target.cpu.activation.env]
SELECTION = "cpu"

[target.cpu.tasks]
cpu-only = "echo cpu-only"
""")


def test_manifest_export_selects_platform_from_lock(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _bounded_gpu_workspace(tmp_pixi_workspace)
    verify_cli_command([pixi, "lock"], cwd=tmp_pixi_workspace)
    exported = verify_cli_command(
        [pixi, "workspace", "export", "conda-environment"],
        env={"CONDA_OVERRIDE_CUDA": "12.5"},
        cwd=tmp_pixi_workspace,
    )
    assert "cpu-package" in exported.stdout
    assert "SELECTION: cpu" in exported.stdout


def test_diagnostics_skip_declared_match_rejected_by_lock(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """A platform matching by declaration alone must not be reported when its
    locked requirements exclude the host, because `pixi run` would skip it."""
    _bounded_gpu_workspace(tmp_pixi_workspace)
    verify_cli_command([pixi, "lock"], cwd=tmp_pixi_workspace)
    host = {"CONDA_OVERRIDE_CUDA": "12.5"}
    info = json.loads(
        verify_cli_command([pixi, "info", "--json"], env=host, cwd=tmp_pixi_workspace).stdout
    )
    [environment] = info["environments_info"]
    assert environment["selected_platform"]["name"] == "cpu"
    assert environment["tasks"] == ["cpu-only"]
    tasks = json.loads(
        verify_cli_command(
            [pixi, "task", "list", "--json"], env=host, cwd=tmp_pixi_workspace
        ).stdout
    )
    [listed] = tasks
    assert [task["name"] for feature in listed["features"] for task in feature["tasks"]] == [
        "cpu-only"
    ]
    listing = verify_cli_command([pixi, "run"], env=host, cwd=tmp_pixi_workspace)
    assert "cpu-only" in listing.stdout + listing.stderr
    assert "gpu-only" not in listing.stdout + listing.stderr
    verify_cli_command(
        [pixi, "run", "cpu-only"], env=host, cwd=tmp_pixi_workspace, stdout_contains="cpu-only"
    )


def test_task_list_selects_platform_from_lock(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _lock_fallback_workspace(tmp_pixi_workspace)
    verify_cli_command([pixi, "lock"], cwd=tmp_pixi_workspace)
    host = {"CONDA_OVERRIDE_CUDA": ""}
    output = verify_cli_command(
        [pixi, "task", "list"], env=host, cwd=tmp_pixi_workspace, strip_ansi=True
    )
    listed = (output.stdout + output.stderr).split()
    assert "plain-only" in listed
    assert "fancy-only" not in listed

    tasks = json.loads(
        verify_cli_command(
            [pixi, "task", "list", "--json"], env=host, cwd=tmp_pixi_workspace
        ).stdout
    )
    names = {
        task["name"]
        for environment in tasks
        for group in [environment["tasks"], *(f["tasks"] for f in environment["features"])]
        for task in group
    }
    assert names == {"which", "plain-only"}


def test_info_tolerates_unparsable_lock_file(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _bounded_gpu_workspace(tmp_pixi_workspace)
    (tmp_pixi_workspace / "pixi.lock").write_text("<<<<<<< HEAD\nnot: [a lock file\n")
    info = json.loads(
        verify_cli_command(
            [pixi, "info", "--json"],
            env={"CONDA_OVERRIDE_CUDA": "12.5"},
            cwd=tmp_pixi_workspace,
        ).stdout
    )
    [environment] = info["environments_info"]
    assert environment["selected_platform"]["name"] == "gpu"


def test_environment_missing_from_lock_selects_by_declaration(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    _bounded_gpu_workspace(tmp_pixi_workspace)
    verify_cli_command([pixi, "lock"], cwd=tmp_pixi_workspace)
    manifest = tmp_pixi_workspace / "pixi.toml"
    manifest.write_text(manifest.read_text() + "\n[environments]\nnewly-added = []\n")
    host = {"CONDA_OVERRIDE_CUDA": "12.1"}
    info = json.loads(
        verify_cli_command([pixi, "info", "--json"], env=host, cwd=tmp_pixi_workspace).stdout
    )
    environment = next(e for e in info["environments_info"] if e["name"] == "newly-added")
    assert environment["selected_platform"]["name"] == "gpu"
    assert environment["skipped_platforms"] == []


def test_environment_without_dependencies_is_not_reported_as_accidental(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    channel = _channel(tmp_pixi_workspace)
    (tmp_pixi_workspace / "pixi.toml").write_text(f"""
[workspace]
name = "no-dependencies"
channels = ["{channel.url}"]
platforms = [{{ name = "gpu", platform = "linux-64", cuda = "12" }}]

[feature.lint.tasks]
lint = "echo linting"

[environments]
lint = {{ features = ["lint"], no-default-feature = true }}
""")
    host = {"CONDA_OVERRIDE_CUDA": ""}
    verify_cli_command([pixi, "lock"], cwd=tmp_pixi_workspace)
    output = verify_cli_command(
        [pixi, "task", "list", "--summary"],
        env=host,
        cwd=tmp_pixi_workspace,
        strip_ansi=True,
    )
    assert "lint (no dependency): lint" in output.stdout + output.stderr
    verify_cli_command(
        [pixi, "run", "-e", "lint", "lint"],
        env=host,
        cwd=tmp_pixi_workspace,
        stdout_contains="linting",
        stderr_excludes="cuda",
    )
    assert not _marker(tmp_pixi_workspace, "lint").exists()


@pytest.mark.parametrize("no_install", [False, True])
def test_fallback_environment_does_not_make_task_ambiguous(
    pixi: Path, tmp_pixi_workspace: Path, no_install: bool
) -> None:
    channel = _channel(tmp_pixi_workspace)
    (tmp_pixi_workspace / "pixi.toml").write_text(f"""
[workspace]
name = "ambiguity"
channels = ["{channel.url}"]
platforms = [
    {{ name = "gpu", platform = "linux-64", cuda = "12.8" }},
    {{ name = "cpu", platform = "linux-64" }},
]

[tasks]
hello = "echo from-default"

[dependencies]
cpu-package = "*"

[feature.accelerated]
platforms = ["gpu"]

[feature.accelerated.dependencies]
gpu-package = "*"

[feature.accelerated.tasks]
hello = "echo from-accelerated"

[environments]
accelerated = {{ features = ["accelerated"], no-default-feature = true }}
""")
    verify_cli_command(
        [pixi, "run", *(["--no-install"] if no_install else []), "hello"],
        env={"CONDA_OVERRIDE_CUDA": "12.6"},
        cwd=tmp_pixi_workspace,
        stdout_contains="from-default",
    )


@pytest.mark.parametrize("no_install", [False, True])
def test_unavailable_custom_capability_does_not_make_task_ambiguous(
    pixi: Path, tmp_pixi_workspace: Path, no_install: bool
) -> None:
    channel = _channel(tmp_pixi_workspace)
    channel.package("custom-package", ["__test_detector_capability >=3"])
    (tmp_pixi_workspace / "pixi.toml").write_text(f"""
[workspace]
name = "custom-ambiguity"
channels = ["{channel.url}"]
platforms = [
    {{ name = "custom", platform = "linux-64", __test_detector_capability = "3" }},
    {{ name = "cpu", platform = "linux-64" }},
]

[target.custom.dependencies]
custom-package = "*"

[target.cpu.dependencies]
cpu-package = "*"

[tasks]
hello = "echo from-default"

[feature.worker]
platforms = ["custom"]

[feature.worker.tasks]
hello = "echo from-worker"

[environments]
worker = ["worker"]
""")
    verify_cli_command([pixi, "lock"], cwd=tmp_pixi_workspace)
    result = verify_cli_command(
        [pixi, "run", "--frozen", *(["--no-install"] if no_install else []), "hello"],
        cwd=tmp_pixi_workspace,
    )
    assert result.stdout.strip() == "from-default"


@pytest.mark.parametrize("legacy_marker", [False, True])
def test_rerun_keeps_lock_selected_platform_with_identical_declarations(
    pixi: Path, tmp_pixi_workspace: Path, legacy_marker: bool
) -> None:
    """Platforms differing only in dependencies can't be told apart by their
    virtual packages, so the installed prefix must still map back to the one
    the lock selected rather than to the first declared."""
    channel = _channel(tmp_pixi_workspace)
    channel.package("bounded-gpu-package", ["__cuda >=12,<12.2"])
    (tmp_pixi_workspace / "pixi.toml").write_text(f"""
[workspace]
name = "identical-declarations"
channels = ["{channel.url}"]
platforms = [
    {{ name = "bounded", platform = "linux-64", cuda = "12" }},
    {{ name = "unbounded", platform = "linux-64", cuda = "12" }},
]

[target.bounded.dependencies]
bounded-gpu-package = "*"

[target.bounded.activation.env]
SELECTION = "bounded"

[target.unbounded.dependencies]
gpu-package = "*"

[target.unbounded.activation.env]
SELECTION = "unbounded"

[tasks]
which = "echo SELECTED-$SELECTION"
""")
    verify_cli_command([pixi, "lock"], cwd=tmp_pixi_workspace)
    for run in range(2):
        result = verify_cli_command(
            [pixi, "run", "--frozen", "which"],
            env={"CONDA_OVERRIDE_CUDA": "12.5"},
            cwd=tmp_pixi_workspace,
        )
        assert result.stdout.strip() == "SELECTED-unbounded"
        if legacy_marker and run == 0:
            marker_path = _marker(tmp_pixi_workspace)
            marker = json.loads(marker_path.read_text())
            del marker["resolved_platform"]["name"]
            marker_path.write_text(json.dumps(marker))


def test_platform_switch_with_identical_packages_keeps_selected_activation(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    channel = _channel(tmp_pixi_workspace)
    (tmp_pixi_workspace / "pixi.toml").write_text(f"""
[workspace]
name = "identical-packages"
channels = ["{channel.url}"]
platforms = [
    {{ name = "first", platform = "linux-64" }},
    {{ name = "second", platform = "linux-64" }},
]

[dependencies]
cpu-package = "*"

[target.first.activation.env]
SELECTION = "first"

[target.second.activation.env]
SELECTION = "second"

[tasks]
which = "echo SELECTED-$SELECTION"
""")
    for platform, expected in [("first", "first"), ("second", "second"), (None, "second")]:
        args = ["--platform", platform] if platform else []
        result = verify_cli_command(
            [pixi, "run", *args, "which"],
            cwd=tmp_pixi_workspace,
        )
        assert result.stdout.strip() == f"SELECTED-{expected}"


def _lock_fallback_workspace(root: Path, *, order: tuple[str, str] = ("fancy", "plain")) -> None:
    """Neither declaration matches; only the plain alias has runnable locked packages."""
    channel = _channel(root)
    platforms = ",\n".join(
        f'    {{ name = "{name}", platform = "linux-64", cuda = "99" }}' for name in order
    )
    (root / "pixi.toml").write_text(f"""
[workspace]
name = "lock-fallback"
channels = ["{channel.url}"]
platforms = [
{platforms}
]

[tasks]
which = "echo SELECTED-$SELECTION-$SCRIPT_SELECTION"

[target.fancy.dependencies]
gpu-package = "*"

[target.fancy.tasks]
fancy-only = "echo WRONG-FANCY-TASK"

[target.fancy.activation]
scripts = ["fancy.sh"]
env.SELECTION = "fancy"

[target.plain.dependencies]
cpu-package = "*"

[target.plain.tasks]
plain-only = "echo SELECTED-$SELECTION-$SCRIPT_SELECTION"

[target.plain.activation]
scripts = ["plain.sh"]
env.SELECTION = "plain"
""")
    for name in order:
        (root / f"{name}.sh").write_text(f"export SCRIPT_SELECTION={name}\n")


@pytest.mark.parametrize("command", ["run", "hook-text", "hook-json"])
def test_frozen_no_install_uses_locked_alias_everywhere(
    pixi: Path, tmp_pixi_workspace: Path, command: str
) -> None:
    _lock_fallback_workspace(tmp_pixi_workspace)
    verify_cli_command([pixi, "lock"], cwd=tmp_pixi_workspace)
    lock = tmp_pixi_workspace / "pixi.lock"
    before = lock.read_bytes()
    host = {"CONDA_OVERRIDE_CUDA": ""}
    if command == "run":
        output = verify_cli_command(
            [pixi, "run", "--frozen", "--no-install", "plain-only"],
            cwd=tmp_pixi_workspace,
            env=host,
        )
        assert output.stdout == snapshot("SELECTED-plain-plain\n")
    elif command == "hook-json":
        output = verify_cli_command(
            [pixi, "shell-hook", "--frozen", "--no-install", "--json"],
            cwd=tmp_pixi_workspace,
            env=host,
        )
        variables = json.loads(output.stdout)["environment_variables"]
        assert {key: variables[key] for key in ("SELECTION", "SCRIPT_SELECTION")} == snapshot(
            {"SELECTION": "plain", "SCRIPT_SELECTION": "plain"}
        )
    else:
        output = verify_cli_command(
            [pixi, "shell-hook", "--frozen", "--no-install", "--shell", "bash"],
            cwd=tmp_pixi_workspace,
            env=host,
        )
        activated = verify_cli_command(
            ["bash", "--noprofile", "--norc"],
            stdin=output.stdout + '\nprintf "SELECTED-%s-%s\\n" "$SELECTION" "$SCRIPT_SELECTION"\n',
            cwd=tmp_pixi_workspace,
            env=host,
        )
        assert activated.stdout == snapshot("SELECTED-plain-plain\n")
    assert lock.read_bytes() == before
    assert not _marker(tmp_pixi_workspace).exists()


@pytest.mark.filterwarnings("ignore::DeprecationWarning")
def test_frozen_no_install_shell_activates_locked_alias(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    import pty

    _lock_fallback_workspace(tmp_pixi_workspace)
    verify_cli_command([pixi, "lock"], cwd=tmp_pixi_workspace)
    before = (tmp_pixi_workspace / "pixi.lock").read_bytes()
    home = tmp_pixi_workspace / "home"
    home.mkdir()
    # The prompt is printed after pixi has sourced its activation script.
    (home / ".bashrc").write_text("PS1='PLATFORM_TEST_READY> '\n")
    pid, fd = pty.fork()
    if pid == 0:
        os.execve(
            str(pixi),
            [
                str(pixi),
                "shell",
                "--frozen",
                "--no-install",
                "--manifest-path",
                str(tmp_pixi_workspace),
            ],
            dict(os.environ)
            | {"CONDA_OVERRIDE_CUDA": "", "SHELL": "/bin/bash", "HOME": str(home), "TERM": "dumb"},
        )
    output = b""
    sent = False
    exited = False
    eof = False
    deadline = time.monotonic() + 30
    try:
        while time.monotonic() < deadline:
            if not select.select([fd], [], [], max(0, deadline - time.monotonic()))[0]:
                break
            try:
                chunk = os.read(fd, 4096)
            except OSError as error:
                if error.errno != errno.EIO:
                    raise
                eof = True
                break
            if not chunk:
                eof = True
                break
            output += chunk
            if not sent and b"PLATFORM_TEST_READY> " in output:
                os.write(
                    fd,
                    b'printf "ACTIVATED-%s-%s\\n" "$SELECTION" "$SCRIPT_SELECTION"\nexit\n',
                )
                sent = True
        waited, status = os.waitpid(pid, 0 if eof else os.WNOHANG)
        exited = waited == pid
        assert exited, output.decode(errors="replace")
        assert os.waitstatus_to_exitcode(status) == 0, output.decode(errors="replace")
        assert b"ACTIVATED-plain-plain\r\n" in output
    finally:
        if not exited:
            os.kill(pid, signal.SIGKILL)
            os.waitpid(pid, 0)
        os.close(fd)
    assert (tmp_pixi_workspace / "pixi.lock").read_bytes() == before
    assert not _marker(tmp_pixi_workspace).exists()


def test_lock_selected_tasks_reach_info_run_listing_and_typo_suggestions(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    _lock_fallback_workspace(tmp_pixi_workspace)
    verify_cli_command([pixi, "lock"], cwd=tmp_pixi_workspace)
    host = {"CONDA_OVERRIDE_CUDA": ""}
    info = json.loads(
        verify_cli_command([pixi, "info", "--json"], env=host, cwd=tmp_pixi_workspace).stdout
    )
    [environment] = info["environments_info"]
    assert environment["selected_platform"]["name"] == "plain"
    assert environment["selected_platform_source"] == "lock_fallback"
    assert sorted(environment["tasks"]) == snapshot(["plain-only", "which"])
    verify_cli_command(
        [pixi, "info"],
        env=host,
        cwd=tmp_pixi_workspace,
        stderr_excludes="Rosetta",
        stdout_contains=["plain", "lock fallback"],
    )
    listing = verify_cli_command([pixi, "run"], env=host, cwd=tmp_pixi_workspace)
    assert "plain-only" in listing.stdout + listing.stderr
    assert "fancy-only" not in listing.stdout + listing.stderr
    verify_cli_command(
        # spellchecker:off
        [pixi, "run", "--frozen", "--no-install", "plain-onl"],
        # spellchecker:on
        ExitCode.COMMAND_NOT_FOUND,
        env=host,
        cwd=tmp_pixi_workspace,
        stderr_contains="plain-only",
        stderr_excludes="fancy-only",
    )
    assert not _marker(tmp_pixi_workspace).exists()


@pytest.mark.parametrize("lock_state", ["missing", "malformed", "newer", "stale", "unreadable"])
@pytest.mark.parametrize("command", [["info", "--json"], ["task", "list", "--json"], ["run"]])
def test_unresolved_diagnostics_do_not_require_a_current_lock(
    pixi: Path, tmp_pixi_workspace: Path, lock_state: str, command: list[str]
) -> None:
    _lock_fallback_workspace(tmp_pixi_workspace)
    lock = tmp_pixi_workspace / "pixi.lock"
    if lock_state == "malformed":
        lock.write_text("<<<<<<< HEAD\nnot: [a lock file\n")
    elif lock_state == "newer":
        lock.write_text("version: 99999\nenvironments: {}\npackages: []\n")
    elif lock_state == "unreadable":
        lock.mkdir()
    elif lock_state == "stale":
        verify_cli_command([pixi, "lock"], cwd=tmp_pixi_workspace)
        manifest = tmp_pixi_workspace / "pixi.toml"
        manifest.write_text(manifest.read_text() + '\n[dependencies]\nnot-in-any-channel = "*"\n')
    before = lock.read_bytes() if lock.is_file() else None
    output = verify_cli_command(
        [pixi, *command],
        cwd=tmp_pixi_workspace,
        env={"CONDA_OVERRIDE_CUDA": ""},
        stderr_excludes="Rosetta",
    )
    assert "which" in output.stdout + output.stderr
    # The warning keeps the cause and the remedy, not just the summary.
    if lock_state == "malformed":
        assert "pixi.lock`: " in output.stderr
    elif lock_state == "newer":
        assert "help: Maximum supported version" in output.stderr
    if command == ["info", "--json"]:
        [environment] = json.loads(output.stdout)["environments_info"]
        if lock_state != "stale":
            assert environment["selected_platform"] is None
            assert environment["selected_platform_source"] is None
    if before is not None:
        assert lock.read_bytes() == before
    elif lock_state == "unreadable":
        assert lock.is_dir()
    else:
        assert not lock.exists()
    assert not _marker(tmp_pixi_workspace).exists()


@pytest.mark.parametrize(
    "command", [["run", "which"], ["shell-hook"], ["shell-hook", "--json"], ["shell"]]
)
def test_no_install_rejects_lock_without_runnable_alias(
    pixi: Path, tmp_pixi_workspace: Path, command: list[str]
) -> None:
    _lock_fallback_workspace(tmp_pixi_workspace)
    manifest = tmp_pixi_workspace / "pixi.toml"
    manifest.write_text(manifest.read_text().replace('cpu-package = "*"', 'gpu-package = "*"'))
    verify_cli_command([pixi, "lock"], cwd=tmp_pixi_workspace)
    lock = tmp_pixi_workspace / "pixi.lock"
    before = lock.read_bytes()
    verify_cli_command(
        [pixi, command[0], "--frozen", "--no-install", *command[1:]],
        ExitCode.FAILURE,
        cwd=tmp_pixi_workspace,
        env={"CONDA_OVERRIDE_CUDA": ""},
        stdout_excludes="SELECTED-",
        stderr_contains="platform",
    )
    assert lock.read_bytes() == before
    assert not _marker(tmp_pixi_workspace).exists()


@pytest.mark.parametrize("order", [("fancy", "plain"), ("plain", "fancy")])
def test_lock_fallback_preserves_alias_declaration_order(
    pixi: Path, tmp_pixi_workspace: Path, order: tuple[str, str]
) -> None:
    _lock_fallback_workspace(tmp_pixi_workspace, order=order)
    manifest = tmp_pixi_workspace / "pixi.toml"
    manifest.write_text(manifest.read_text().replace('gpu-package = "*"', 'cpu-package = "*"'))
    verify_cli_command([pixi, "lock"], cwd=tmp_pixi_workspace)
    output = verify_cli_command(
        [pixi, "run", "--frozen", "--no-install", "which"],
        cwd=tmp_pixi_workspace,
        env={"CONDA_OVERRIDE_CUDA": ""},
    )
    assert output.stdout == f"SELECTED-{order[0]}-{order[0]}\n"


def test_reused_prefix_wins_until_lock_change_rematerializes_it(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    _lock_fallback_workspace(tmp_pixi_workspace)
    verify_cli_command(
        [pixi, "run", "which"],
        cwd=tmp_pixi_workspace,
        env={"CONDA_OVERRIDE_CUDA": ""},
        stdout_contains="SELECTED-plain-plain",
    )
    host = {"CONDA_OVERRIDE_CUDA": "99"}
    marker = _marker(tmp_pixi_workspace).read_bytes()
    for args in (["run", "--frozen", "--no-install", "which"], ["run", "--frozen", "which"]):
        verify_cli_command(
            [pixi, *args],
            cwd=tmp_pixi_workspace,
            env=host,
            stdout_contains="SELECTED-plain-plain",
        )
    info = json.loads(
        verify_cli_command([pixi, "info", "--json"], cwd=tmp_pixi_workspace, env=host).stdout
    )
    assert info["environments_info"][0]["selected_platform_source"] == "installed"
    assert _marker(tmp_pixi_workspace).read_bytes() == marker
    manifest = tmp_pixi_workspace / "pixi.toml"
    manifest.write_text(manifest.read_text() + '\n[dependencies]\ngpu-package = "*"\n')
    verify_cli_command(
        [pixi, "run", "which"],
        cwd=tmp_pixi_workspace,
        env=host,
        stdout_contains="SELECTED-fancy-fancy",
    )
    assert (
        json.loads(_marker(tmp_pixi_workspace).read_text())["resolved_platform"]["name"] == "fancy"
    )
    hook = json.loads(
        verify_cli_command(
            [pixi, "shell-hook", "--frozen", "--no-install", "--json"],
            cwd=tmp_pixi_workspace,
            env=host,
        ).stdout
    )
    assert hook["environment_variables"]["SELECTION"] == "fancy"


@pytest.mark.parametrize("dependencies", [False, True])
def test_empty_platforms_baseline_requires_dependency_free_environment(
    pixi: Path, tmp_pixi_workspace: Path, dependencies: bool
) -> None:
    channel = _channel(tmp_pixi_workspace)
    manifest = tmp_pixi_workspace / "pixi.toml"
    manifest.write_text(
        f"""
[workspace]
name = "empty-platforms"
channels = ["{channel.url}"]
platforms = []

[tasks]
hello = "echo HOST-BASELINE"
"""
        + ('\n[dependencies]\ncpu-package = "*"\n' if dependencies else "")
    )
    verify_cli_command([pixi, "lock"], cwd=tmp_pixi_workspace)
    output = verify_cli_command(
        [pixi, "run", "--frozen", "--no-install", "hello"],
        ExitCode.FAILURE if dependencies else ExitCode.SUCCESS,
        cwd=tmp_pixi_workspace,
    )
    if dependencies:
        assert "HOST-BASELINE" not in output.stdout
    else:
        assert output.stdout == snapshot("HOST-BASELINE\n")
        info = json.loads(
            verify_cli_command([pixi, "info", "--json"], cwd=tmp_pixi_workspace).stdout
        )
        assert info["environments_info"][0]["selected_platform_source"] == "host_baseline"


def test_explicit_platform_is_checked_for_task_dependency_environment(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    (tmp_pixi_workspace / "pixi.toml").write_text("""
[workspace]
name = "per-environment-explicit"
channels = []
platforms = [
    { name = "first", platform = "linux-64" },
    { name = "second", platform = "linux-64" },
]

[tasks]
root = { cmd = "echo ROOT-RAN", depends-on = [{ task = "child", environment = "worker" }] }

[feature.worker]
platforms = ["second"]

[feature.worker.tasks]
child = "echo CHILD-RAN"

[environments]
worker = ["worker"]
""")
    verify_cli_command([pixi, "lock"], cwd=tmp_pixi_workspace)
    verify_cli_command(
        [pixi, "run", "--frozen", "--no-install", "--platform", "first", "root"],
        ExitCode.FAILURE,
        cwd=tmp_pixi_workspace,
        stderr_contains=["first", "worker"],
        stdout_excludes=["ROOT-RAN", "CHILD-RAN"],
    )
    verify_cli_command(
        [pixi, "run", "--frozen", "--no-install", "--platform", "second", "root"],
        cwd=tmp_pixi_workspace,
        stdout_contains=["ROOT-RAN", "CHILD-RAN"],
    )


@pytest.mark.parametrize("mode", ["default", "locked", "frozen"])
@pytest.mark.parametrize("command", [["run", "which"], ["shell-hook", "--json"]])
def test_no_install_preserves_lock_mode_semantics(
    pixi: Path, tmp_pixi_workspace: Path, mode: str, command: list[str]
) -> None:
    _lock_fallback_workspace(tmp_pixi_workspace)
    verify_cli_command([pixi, "lock"], cwd=tmp_pixi_workspace)
    lock = tmp_pixi_workspace / "pixi.lock"
    before = lock.read_bytes()
    manifest = tmp_pixi_workspace / "pixi.toml"
    manifest.write_text(manifest.read_text().replace('gpu-package = "*"', 'cpu-package = "*"'))
    output = verify_cli_command(
        [
            pixi,
            command[0],
            "--no-install",
            *([] if mode == "default" else [f"--{mode}"]),
            *command[1:],
        ],
        ExitCode.FAILURE if mode == "locked" else ExitCode.SUCCESS,
        cwd=tmp_pixi_workspace,
        env={"CONDA_OVERRIDE_CUDA": ""},
    )
    if mode == "default":
        assert lock.read_bytes() != before
        assert "fancy" in output.stdout
    else:
        assert lock.read_bytes() == before
        if mode == "frozen":
            assert "plain" in output.stdout
    assert not _marker(tmp_pixi_workspace).exists()


def test_lock_selected_target_task_is_found_in_its_named_feature(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    _lock_fallback_workspace(tmp_pixi_workspace)
    manifest = tmp_pixi_workspace / "pixi.toml"
    manifest.write_text(
        manifest.read_text()
        .replace("[target.plain.tasks]", "[feature.tools.target.plain.tasks]")
        .replace("[target.fancy.tasks]", "[feature.tools.target.fancy.tasks]")
        + '\n[environments]\ntools = ["tools"]\n'
    )
    verify_cli_command([pixi, "lock"], cwd=tmp_pixi_workspace)
    host = {"CONDA_OVERRIDE_CUDA": ""}
    output = verify_cli_command(
        [pixi, "run", "--frozen", "--no-install", "plain-only"],
        cwd=tmp_pixi_workspace,
        env=host,
    )
    assert output.stdout == snapshot("SELECTED-plain-plain\n")
    tasks = json.loads(
        verify_cli_command(
            [pixi, "task", "list", "--json"], cwd=tmp_pixi_workspace, env=host
        ).stdout
    )
    tools = next(environment for environment in tasks if environment["environment"] == "tools")
    feature = next(feature for feature in tools["features"] if feature["name"] == "tools")
    assert [task["name"] for task in feature["tasks"]] == snapshot(["plain-only"])
    assert not _marker(tmp_pixi_workspace, "tools").exists()


def test_manifest_export_without_lock_keeps_dependency_specifications(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    (tmp_pixi_workspace / "pixi.toml").write_text("""
[workspace]
name = "unlocked-export"
channels = []
platforms = ["linux-64"]

[dependencies]
python = ">=3.12"
""")
    output = verify_cli_command(
        [pixi, "workspace", "export", "conda-environment"], cwd=tmp_pixi_workspace
    )
    assert yaml.safe_load(output.stdout)["dependencies"] == ["python >=3.12"]
    assert not (tmp_pixi_workspace / "pixi.lock").exists()
