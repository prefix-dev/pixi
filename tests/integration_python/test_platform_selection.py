"""Platform selection across installation, inspection, scripts, and task dependencies."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from .common import CURRENT_PLATFORM, verify_cli_command
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


def test_task_dependency_uses_its_own_selected_platform_and_activation(
    pixi: Path, tmp_pixi_workspace: Path
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
        [pixi, "run", "--frozen", "--manifest-path", manifest, "root"],
        env={"CONDA_OVERRIDE_CUDA": "12.2"},
        stdout_contains=["ROOT-gpu", "CHILD-cpu"],
        stdout_excludes="WRONG-GPU-CHILD",
    )
    default_marker = json.loads(_marker(tmp_pixi_workspace).read_text())
    worker_marker = json.loads(_marker(tmp_pixi_workspace, "worker").read_text())
    assert any(
        value.startswith("__cuda=")
        for value in default_marker["resolved_platform"]["virtual_packages"]
    )
    assert not any(
        value.startswith("__cuda=")
        for value in worker_marker["resolved_platform"]["virtual_packages"]
    )


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


def test_task_list_selects_platform_from_lock(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _bounded_gpu_workspace(tmp_pixi_workspace)
    verify_cli_command([pixi, "lock"], cwd=tmp_pixi_workspace)
    host = {"CONDA_OVERRIDE_CUDA": "12.5"}
    output = verify_cli_command(
        [pixi, "task", "list"], env=host, cwd=tmp_pixi_workspace, strip_ansi=True
    )
    listed = (output.stdout + output.stderr).split()
    assert "cpu-only" in listed
    assert "gpu-only" not in listed

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
    assert names == {"cpu-only"}


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


def test_fallback_environment_does_not_make_task_ambiguous(
    pixi: Path, tmp_pixi_workspace: Path
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
        [pixi, "run", "hello"],
        env={"CONDA_OVERRIDE_CUDA": "12.6"},
        cwd=tmp_pixi_workspace,
        stdout_contains="from-default",
    )


def test_unavailable_custom_capability_does_not_make_task_ambiguous(
    pixi: Path, tmp_pixi_workspace: Path
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
        [pixi, "run", "--frozen", "hello"],
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
