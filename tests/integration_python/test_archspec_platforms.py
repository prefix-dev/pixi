"""End-to-end tests for DAG-aware ``__archspec`` handling.

A workspace declares an ``x86_64_v3`` variant of the host subdir next to the
bare subdir, and ``CONDA_OVERRIDE_ARCHSPEC`` simulates hosts of different
capability so the tests are deterministic on any machine. The
``microarch-v3`` package in the in-repo ``virtual_packages`` channel requires
``__archspec ==1 x86_64_v3``, mirroring conda-forge's microarchitecture
metapackages. All tests stay network-free.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from .common import CURRENT_PLATFORM, ExitCode, verify_cli_command

# The tests declare an x86_64_v3 variant of the host subdir, which only makes
# sense where the subdir's baseline microarchitecture is x86_64.
X86_64_SUBDIRS = {"linux-64", "win-64", "osx-64"}

requires_x86_64_subdir = pytest.mark.skipif(
    CURRENT_PLATFORM not in X86_64_SUBDIRS,
    reason="the workspace declares an x86_64_v3 variant of the host subdir",
)


def _write_workspace(workspace: Path, channel: str, *, include_baseline: bool) -> Path:
    baseline = f'\n  "{CURRENT_PLATFORM}",' if include_baseline else ""
    manifest = workspace / "pixi.toml"
    manifest.write_text(
        f"""
[workspace]
name = "archspec-variants"
channels = ["{channel}"]
platforms = [
  {{ name = "v3", platform = "{CURRENT_PLATFORM}", archspec = "x86_64_v3" }},{baseline}
]

[tasks]
hi = "echo hi"

[target.v3.dependencies]
microarch-v3 = "*"
"""
    )
    return manifest


def _resolved_virtual_packages(workspace: Path) -> list[str]:
    marker = workspace / ".pixi" / "envs" / "default" / "conda-meta" / "pixi"
    return json.loads(marker.read_text())["resolved_platform"]["virtual_packages"]


def _microarch_v3_installed(workspace: Path) -> bool:
    conda_meta = workspace / ".pixi" / "envs" / "default" / "conda-meta"
    return any(entry.name.startswith("microarch-v3-") for entry in conda_meta.glob("*.json"))


@requires_x86_64_subdir
def test_capable_host_selects_archspec_variant(
    pixi: Path, tmp_pixi_workspace: Path, virtual_packages_channel: str
) -> None:
    """A host whose microarchitecture implements the declared baseline picks
    the variant (declared first) and installs its packages."""
    manifest = _write_workspace(tmp_pixi_workspace, virtual_packages_channel, include_baseline=True)
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
        ExitCode.SUCCESS,
        env={"CONDA_OVERRIDE_ARCHSPEC": "skylake"},
    )
    assert "__archspec=0=x86_64_v3" in _resolved_virtual_packages(tmp_pixi_workspace)
    assert _microarch_v3_installed(tmp_pixi_workspace)


@requires_x86_64_subdir
def test_baseline_host_falls_back_to_bare_subdir(
    pixi: Path, tmp_pixi_workspace: Path, virtual_packages_channel: str
) -> None:
    """A host below the variant's baseline falls through to the bare subdir
    declared after it, without the variant's packages."""
    manifest = _write_workspace(tmp_pixi_workspace, virtual_packages_channel, include_baseline=True)
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
        ExitCode.SUCCESS,
        env={"CONDA_OVERRIDE_ARCHSPEC": "x86_64"},
    )
    assert "__archspec=0=x86_64_v3" not in _resolved_virtual_packages(tmp_pixi_workspace)
    assert not _microarch_v3_installed(tmp_pixi_workspace)


@requires_x86_64_subdir
def test_incapable_host_is_refused_with_override_hint(
    pixi: Path, tmp_pixi_workspace: Path, virtual_packages_channel: str
) -> None:
    """Without a fallback platform, a host below the baseline is refused and
    told exactly which override would vouch for the machine."""
    manifest = _write_workspace(
        tmp_pixi_workspace, virtual_packages_channel, include_baseline=False
    )
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
        ExitCode.FAILURE,
        env={"CONDA_OVERRIDE_ARCHSPEC": "x86_64"},
        stderr_contains="CONDA_OVERRIDE_ARCHSPEC=x86_64_v3",
        strip_ansi=True,
    )


@requires_x86_64_subdir
def test_run_refuses_weaker_machine_than_installed_for(
    pixi: Path, tmp_pixi_workspace: Path, virtual_packages_channel: str
) -> None:
    """An environment installed for the x86_64_v3 variant refuses to run on a
    machine that only provides the x86_64 baseline: the installed packages
    require `__archspec ==1 x86_64_v3`."""
    manifest = _write_workspace(
        tmp_pixi_workspace, virtual_packages_channel, include_baseline=False
    )
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
        ExitCode.SUCCESS,
        env={"CONDA_OVERRIDE_ARCHSPEC": "x86_64_v3"},
    )
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "hi"],
        ExitCode.FAILURE,
        env={"CONDA_OVERRIDE_ARCHSPEC": "x86_64"},
        stderr_contains="__archspec",
        strip_ansi=True,
    )


def test_add_rejects_unknown_archspec_name(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """The CLI rejects archspec names the database does not know, with a
    did-you-mean hint for the dashed spelling."""
    manifest = tmp_pixi_workspace / "pixi.toml"
    manifest.write_text(
        f"""
[workspace]
name = "archspec-reject"
channels = []
platforms = ["{CURRENT_PLATFORM}"]
"""
    )
    verify_cli_command(
        [
            pixi,
            "workspace",
            "--manifest-path",
            manifest,
            "platform",
            "add",
            f"v3={CURRENT_PLATFORM}",
            "--archspec",
            "x86-64-v3",
            "--no-install",
        ],
        ExitCode.FAILURE,
        stderr_contains="did you mean 'x86_64_v3'",
        strip_ansi=True,
    )
