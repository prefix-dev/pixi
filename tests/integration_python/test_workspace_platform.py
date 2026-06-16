"""Surface-area tests for `pixi workspace platform`.

These tests exercise the CLI added by the "Add command line interface to
interact with platforms" / "support richer platforms with virtual packages"
commits. Heavy install/publish flows that don't fit pytest live in
`~/Downloads/platform_test.py`; this module focuses on:

- argument parsing for add/edit/list/remove/show
- TOML round-trip of bare-string vs inline-table platforms
- virtual-package upsert / remove / clear semantics
- lockfile invariants that are observable without a real solve (the platforms
  block at the top of `pixi.lock` is rewritten regardless of `--no-install`)

To keep the suite fast everything uses `--no-install` and a manifest with no
channels/dependencies so no network is involved.
"""

from __future__ import annotations

import json
import sys
import tomllib
from pathlib import Path
from typing import Any

import pytest
import yaml

from .common import CURRENT_PLATFORM, ExitCode, verify_cli_command


# ----------------------------------------------------------------------------
# helpers
# ----------------------------------------------------------------------------


def _seed_workspace(path: Path, platforms: list[str] | None = None) -> Path:
    """Write a minimal `pixi.toml` and return its path.

    Uses no channels and no dependencies so `--no-install` solves trivially
    and never hits the network.
    """
    if platforms is None:
        platforms = [CURRENT_PLATFORM]
    platforms_inline = ", ".join(f'"{p}"' for p in platforms)
    manifest = path / "pixi.toml"
    manifest.write_text(
        f"""\
[workspace]
name = "platform-test"
channels = []
platforms = [{platforms_inline}]
"""
    )
    return manifest


def _platforms_from_toml(manifest: Path) -> list[str | dict[str, Any]]:
    """Parse `[workspace].platforms` and return entries as Python data.

    Bare-string entries come back as `str`, inline-table entries as `dict`.
    """
    data = tomllib.loads(manifest.read_text())
    return data["workspace"]["platforms"]


def _lockfile_platforms(workspace_dir: Path) -> list[str | dict[str, Any]]:
    """Read the `platforms:` block at the top of `pixi.lock`."""
    lock = workspace_dir / "pixi.lock"
    assert lock.exists(), f"expected lockfile at {lock}"
    data = yaml.safe_load(lock.read_text())
    return data.get("platforms", [])


def _run_platform(
    pixi: Path,
    workspace: Path,
    *args: str,
    expected_exit_code: ExitCode = ExitCode.SUCCESS,
    stdout_contains: list[str] | str | None = None,
    stderr_contains: list[str] | str | None = None,
    stdout_excludes: list[str] | str | None = None,
):
    """Run `pixi workspace platform <args>` against a temp workspace."""
    return verify_cli_command(
        [
            str(pixi),
            "workspace",
            "--manifest-path",
            str(workspace / "pixi.toml"),
            "platform",
            *args,
        ],
        expected_exit_code=expected_exit_code,
        stdout_contains=stdout_contains,
        stderr_contains=stderr_contains,
        stdout_excludes=stdout_excludes,
        # Strip ANSI so we can match against the actual text without colour
        # codes interfering. The CLI emits colour by default.
        strip_ansi=True,
    )


# ----------------------------------------------------------------------------
# add
# ----------------------------------------------------------------------------


def test_add_single_bare_subdir(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(pixi, tmp_pixi_workspace, "add", "linux-64", "--no-install")
    platforms = _platforms_from_toml(tmp_pixi_workspace / "pixi.toml")
    # Bare subdir should round-trip as a string, not an inline table.
    assert "linux-64" in platforms
    assert all(isinstance(p, str) or p.get("name") != "linux-64" for p in platforms)


def test_add_multiple_bare_subdirs(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "add",
        "linux-64",
        "osx-arm64",
        "win-64",
        "--no-install",
    )
    platforms = _platforms_from_toml(tmp_pixi_workspace / "pixi.toml")
    for name in ("linux-64", "osx-arm64", "win-64"):
        assert name in platforms


def test_add_alias_a_works(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(pixi, tmp_pixi_workspace, "a", "osx-64", "--no-install")
    assert "osx-64" in _platforms_from_toml(tmp_pixi_workspace / "pixi.toml")


def test_add_custom_name_with_subdir(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(pixi, tmp_pixi_workspace, "add", "gpu-linux=linux-64", "--no-install")
    platforms = _platforms_from_toml(tmp_pixi_workspace / "pixi.toml")
    entry = next(p for p in platforms if isinstance(p, dict) and p["name"] == "gpu-linux")
    assert entry["platform"] == "linux-64"
    # No virtual-package shortcut keys should leak in when none were requested.
    for vp_key in ("cuda", "archspec", "glibc", "linux", "macos", "windows"):
        assert vp_key not in entry


def test_add_custom_name_with_cuda(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "add",
        "gpu-linux=linux-64",
        "--cuda",
        "12.0",
        "--no-install",
    )
    platforms = _platforms_from_toml(tmp_pixi_workspace / "pixi.toml")
    entry = next(p for p in platforms if isinstance(p, dict) and p["name"] == "gpu-linux")
    assert entry["platform"] == "linux-64"
    assert entry["cuda"] == "12.0"


def test_add_custom_name_with_glibc_on_linux(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "add",
        "modern-linux=linux-64",
        "--glibc",
        "2.40",
        "--no-install",
    )
    entry = next(
        p
        for p in _platforms_from_toml(tmp_pixi_workspace / "pixi.toml")
        if isinstance(p, dict) and p["name"] == "modern-linux"
    )
    # `--glibc` shortcut writes the `glibc` key (mapped to `__glibc` internally).
    # Use a non-default value (the linux-64 default `__glibc` is elided).
    assert entry["glibc"] == "2.40"


def test_add_glibc_on_windows_rejected(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "add",
        "weird-win=win-64",
        "--glibc",
        "2.28",
        "--no-install",
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains="--glibc only applies to linux subdirs",
    )


def test_add_linux_macos_windows_friendly_flags(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """The three subdir-family flags (`--linux`, `--macos`, `--windows`) each
    declare their `__linux`/`__osx`/`__win` virtual package and write the
    friendly key into TOML on the right subdir family."""
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(
        pixi, tmp_pixi_workspace, "add", "modern-linux=linux-64", "--linux", "5.10", "--no-install"
    )
    _run_platform(
        pixi, tmp_pixi_workspace, "add", "modern-mac=osx-arm64", "--macos", "14.0", "--no-install"
    )
    _run_platform(
        pixi, tmp_pixi_workspace, "add", "modern-win=win-64", "--windows", "11", "--no-install"
    )
    entries = {
        p["name"]: p
        for p in _platforms_from_toml(tmp_pixi_workspace / "pixi.toml")
        if isinstance(p, dict)
    }
    assert entries["modern-linux"]["linux"] == "5.10"
    assert entries["modern-mac"]["macos"] == "14.0"
    assert entries["modern-win"]["windows"] == "11"


@pytest.mark.parametrize(
    ("flag", "value", "wrong_subdir", "family"),
    [
        ("--linux", "5.10", "win-64", "linux"),
        ("--macos", "14.0", "linux-64", "osx"),
        ("--windows", "10", "linux-64", "win"),
    ],
)
def test_add_family_flag_subdir_restriction(
    pixi: Path,
    tmp_pixi_workspace: Path,
    flag: str,
    value: str,
    wrong_subdir: str,
    family: str,
) -> None:
    """Each family flag rejects subdirs outside its family, the same way
    `--glibc` already does for non-linux subdirs."""
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "add",
        f"wrong={wrong_subdir}",
        flag,
        value,
        "--no-install",
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains=f"{flag} only applies to {family} subdirs",
    )


def test_add_archspec_build_string(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "add",
        "v3-linux=linux-64",
        "--archspec",
        "x86_64_v3",
        "--no-install",
    )
    entry = next(
        p
        for p in _platforms_from_toml(tmp_pixi_workspace / "pixi.toml")
        if isinstance(p, dict) and p["name"] == "v3-linux"
    )
    # archspec carries the microarchitecture string.
    assert entry["archspec"] == "x86_64_v3"


def test_add_raw_virtual_package_repeated(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Raw virtual-package specs are passed as trailing `__name=value`
    positionals, mirroring the `__name = "..."` escape hatch in pixi.toml."""
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "add",
        "rich-linux=linux-64",
        "__cuda=12.0",
        "__glibc=2.40",
        "--no-install",
    )
    entry = next(
        p
        for p in _platforms_from_toml(tmp_pixi_workspace / "pixi.toml")
        if isinstance(p, dict) and p["name"] == "rich-linux"
    )
    assert entry["cuda"] == "12.0"
    assert entry["glibc"] == "2.40"


def test_add_duplicate_virtual_package_rejected(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """`--cuda` and a `__cuda=...` raw positional together should error."""
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "add",
        "gpu-linux=linux-64",
        "__cuda=11.0",
        "--cuda",
        "12.0",
        "--no-install",
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains="more than once",
    )


def test_add_duplicate_platform_rejected(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """The same platform passed twice should error rather than silently collapse."""
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "add",
        "osx-arm64",
        "osx-arm64",
        "--no-install",
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains="more than once",
    )


def test_add_invalid_virtual_package_name(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """A trailing positional that doesn't start with `__` is treated as a
    second platform entry, which then trips the single-platform-with-vps rule."""
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "add",
        "weird=linux-64",
        "--cuda",
        "12.0",
        "cuda=12.0",
        "--no-install",
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains="exactly one platform",
    )


def test_add_invalid_subdir(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "add",
        "bogus-subdir",
        "--no-install",
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains="bogus-subdir",
    )


def test_add_bare_subdir_plus_vp_rejected(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Adding virtual packages requires a custom platform name, not a bare subdir."""
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "add",
        "linux-64",
        "--cuda",
        "12.0",
        "--no-install",
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains="virtual packages require a custom platform name",
    )


def test_add_vp_with_multiple_positionals_rejected(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "add",
        "linux-64",
        "osx-64",
        "--cuda",
        "12.0",
        "--no-install",
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains="exactly one platform",
    )


@pytest.mark.parametrize("reserved", ["linux", "unix", "win", "osx"])
def test_add_reserved_family_name_rejected(
    pixi: Path, tmp_pixi_workspace: Path, reserved: str
) -> None:
    """Family target selectors can't be used as platform names."""
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "add",
        f"{reserved}=linux-64",
        "--no-install",
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains="reserved",
    )


def test_add_invalid_platform_name_rejected(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "add",
        "bad name=linux-64",
        "--no-install",
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains="invalid platform name",
    )


def test_add_to_named_feature(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """`--feature gpu` should write into `[feature.gpu] platforms`, not workspace."""
    manifest = _seed_workspace(tmp_pixi_workspace, [CURRENT_PLATFORM])
    # Seed an empty feature so the toml has a place to land.
    manifest.write_text(
        manifest.read_text() + '\n[feature.gpu]\nplatforms = []\n[environments]\ngpu = ["gpu"]\n'
    )
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "add",
        "linux-64",
        "--feature",
        "gpu",
        "--no-install",
    )
    data = tomllib.loads(manifest.read_text())
    assert "linux-64" in data["feature"]["gpu"]["platforms"]


def test_add_rich_platform_to_named_feature(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """`--feature` and a virtual-package flag compose: the rich platform
    lands in both the workspace's platforms list (as an inline table) and
    the feature's platforms list (as a bare name reference)."""
    manifest = _seed_workspace(tmp_pixi_workspace, [CURRENT_PLATFORM])
    manifest.write_text(
        manifest.read_text() + '\n[feature.gpu]\nplatforms = []\n[environments]\ngpu = ["gpu"]\n'
    )
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "add",
        "gpu-linux=linux-64",
        "--cuda",
        "12.0",
        "--feature",
        "gpu",
        "--no-install",
    )
    data = tomllib.loads(manifest.read_text())
    # Feature lists the platform by name.
    assert "gpu-linux" in data["feature"]["gpu"]["platforms"]
    # Workspace got the rich entry with the declared VP.
    rich = next(
        p
        for p in _platforms_from_toml(manifest)
        if isinstance(p, dict) and p.get("name") == "gpu-linux"
    )
    assert rich["platform"] == "linux-64"
    assert rich["cuda"] == "12.0"


# ----------------------------------------------------------------------------
# lockfile invalidation: adding/removing platforms must rewrite pixi.lock
# ----------------------------------------------------------------------------


def test_lockfile_gets_new_platform(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(pixi, tmp_pixi_workspace, "add", "linux-64", "--no-install")
    lock_platforms = _lockfile_platforms(tmp_pixi_workspace)
    # Lockfile lists either bare strings or {name, subdir, ...} dicts.
    names = [p if isinstance(p, str) else p["name"] for p in lock_platforms]
    assert "linux-64" in names


def test_lockfile_records_custom_platform_and_vps(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "add",
        "gpu-linux=linux-64",
        "--cuda",
        "12.0",
        "--no-install",
    )
    lock_platforms = _lockfile_platforms(tmp_pixi_workspace)
    # Rich platforms are written under a short alias (e.g. `p1`) rather than
    # their manifest name; they are matched back to `gpu-linux` by identity
    # (subdir + virtual packages) when the lock file is read.
    entry = next(
        p
        for p in lock_platforms
        if isinstance(p, dict) and "__cuda=12.0" in p.get("virtual-packages", [])
    )
    assert entry["subdir"] == "linux-64"
    assert entry["name"] != "gpu-linux"
    assert entry["name"].startswith("p")


def test_lockfile_records_removed_platform_lazy_pruning(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """`platform remove --no-install` updates pixi.toml but leaves the
    top-level `platforms:` block of `pixi.lock` alone -- pruning happens
    lazily on the next satisfiability divergence (an env that actually
    references the removed platform). The manifest must still reflect the
    removal."""
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(pixi, tmp_pixi_workspace, "add", "linux-64", "osx-64", "--no-install")
    _run_platform(pixi, tmp_pixi_workspace, "remove", "osx-64", "--no-install")

    # Manifest is the source of truth -- removed platform must be gone.
    names_in_manifest = [
        p if isinstance(p, str) else p["name"]
        for p in _platforms_from_toml(tmp_pixi_workspace / "pixi.toml")
    ]
    assert "osx-64" not in names_in_manifest
    assert "linux-64" in names_in_manifest

    # Lockfile still lists both platforms; this is the documented lazy-prune
    # behavior also present in pixi 0.68.1, not a regression of the new CLI.
    lock_names = [
        p if isinstance(p, str) else p["name"] for p in _lockfile_platforms(tmp_pixi_workspace)
    ]
    assert "linux-64" in lock_names


# ----------------------------------------------------------------------------
# edit
# ----------------------------------------------------------------------------


def _seed_with_rich_platform(workspace: Path, pixi: Path) -> None:
    """Helper: workspace with a custom `gpu-linux` platform carrying __cuda=11.0."""
    _seed_workspace(workspace)
    _run_platform(
        pixi,
        workspace,
        "add",
        "gpu-linux=linux-64",
        "--cuda",
        "11.0",
        "--no-install",
    )


def test_edit_replaces_vp_version(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_with_rich_platform(tmp_pixi_workspace, pixi)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "edit",
        "gpu-linux",
        "--cuda",
        "12.4",
        "--no-install",
    )
    entry = next(
        p
        for p in _platforms_from_toml(tmp_pixi_workspace / "pixi.toml")
        if isinstance(p, dict) and p["name"] == "gpu-linux"
    )
    assert entry["cuda"] == "12.4"


def test_edit_add_second_vp(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_with_rich_platform(tmp_pixi_workspace, pixi)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "edit",
        "gpu-linux",
        "--glibc",
        "2.40",
        "--no-install",
    )
    entry = next(
        p
        for p in _platforms_from_toml(tmp_pixi_workspace / "pixi.toml")
        if isinstance(p, dict) and p["name"] == "gpu-linux"
    )
    assert entry["cuda"] == "11.0"
    assert entry["glibc"] == "2.40"


def test_edit_remove_named_vp(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_with_rich_platform(tmp_pixi_workspace, pixi)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "edit",
        "gpu-linux",
        "--glibc",
        "2.40",
        "--no-install",
    )
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "edit",
        "gpu-linux",
        "--remove-virtual-package",
        "__cuda",
        "--no-install",
    )
    entry = next(
        p
        for p in _platforms_from_toml(tmp_pixi_workspace / "pixi.toml")
        if isinstance(p, dict) and p["name"] == "gpu-linux"
    )
    assert entry["glibc"] == "2.40"
    assert "cuda" not in entry


def test_edit_clear_then_upsert(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_with_rich_platform(tmp_pixi_workspace, pixi)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "edit",
        "gpu-linux",
        "--clear-virtual-packages",
        "--archspec",
        "x86_64_v3",
        "--no-install",
    )
    entry = next(
        p
        for p in _platforms_from_toml(tmp_pixi_workspace / "pixi.toml")
        if isinstance(p, dict) and p["name"] == "gpu-linux"
    )
    assert entry["archspec"] == "x86_64_v3"
    assert "cuda" not in entry


def test_edit_set_subdir(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_with_rich_platform(tmp_pixi_workspace, pixi)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "edit",
        "gpu-linux",
        "--subdir",
        "linux-aarch64",
        "--no-install",
    )
    entry = next(
        p
        for p in _platforms_from_toml(tmp_pixi_workspace / "pixi.toml")
        if isinstance(p, dict) and p["name"] == "gpu-linux"
    )
    assert entry["platform"] == "linux-aarch64"
    # VP declaration survives an unrelated subdir change.
    assert entry["cuda"] == "11.0"


def test_edit_noop_rejected(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_with_rich_platform(tmp_pixi_workspace, pixi)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "edit",
        "gpu-linux",
        "--no-install",
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains="nothing to do",
    )


def test_edit_subdir_platform_transitions_to_rich(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Editing a bare subdir platform to add a virtual package transitions it
    into a rich platform, auto-renamed away from the bare subdir form."""
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(pixi, tmp_pixi_workspace, "add", "linux-64", "--no-install")
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "edit",
        "linux-64",
        "--cuda",
        "12.0",
        "--no-install",
    )
    platforms = _platforms_from_toml(tmp_pixi_workspace / "pixi.toml")
    # The bare `linux-64` string entry is gone; it is now a rich linux-64
    # platform carrying the requested `__cuda`.
    assert "linux-64" not in platforms
    entry = next(p for p in platforms if isinstance(p, dict) and p["platform"] == "linux-64")
    assert entry["cuda"] == "12.0"


def test_edit_unknown_platform_rejected(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "edit",
        "does-not-exist",
        "--cuda",
        "12.0",
        "--no-install",
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains="does-not-exist",
    )


# ----------------------------------------------------------------------------
# move
# ----------------------------------------------------------------------------


def _names(manifest: Path) -> list[str]:
    """Platform names in declaration order (bare strings or table `name`s)."""
    return [p if isinstance(p, str) else p["name"] for p in _platforms_from_toml(manifest)]


def test_move_to_top(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest = _seed_workspace(tmp_pixi_workspace, ["linux-64", "osx-64", "win-64"])
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "move",
        "win-64",
        "--to-top",
        "--no-install",
        stderr_contains="Moved platform win-64",
    )
    assert _names(manifest) == ["win-64", "linux-64", "osx-64"]


def test_move_before_and_after(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest = _seed_workspace(tmp_pixi_workspace, ["linux-64", "osx-64", "win-64"])
    _run_platform(pixi, tmp_pixi_workspace, "move", "win-64", "--before", "osx-64", "--no-install")
    assert _names(manifest) == ["linux-64", "win-64", "osx-64"]
    _run_platform(pixi, tmp_pixi_workspace, "move", "linux-64", "--after", "osx-64", "--no-install")
    assert _names(manifest) == ["win-64", "osx-64", "linux-64"]


def test_move_alias_mv_to_bottom(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest = _seed_workspace(tmp_pixi_workspace, ["linux-64", "osx-64", "win-64"])
    _run_platform(pixi, tmp_pixi_workspace, "mv", "linux-64", "--to-bottom", "--no-install")
    assert _names(manifest) == ["osx-64", "win-64", "linux-64"]


def test_move_requires_an_anchor(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_workspace(tmp_pixi_workspace, ["linux-64", "osx-64"])
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "move",
        "linux-64",
        "--no-install",
        expected_exit_code=ExitCode.INCORRECT_USAGE,
        stderr_contains="--before",
    )


def test_move_anchors_are_mutually_exclusive(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_workspace(tmp_pixi_workspace, ["linux-64", "osx-64"])
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "move",
        "linux-64",
        "--to-top",
        "--to-bottom",
        "--no-install",
        expected_exit_code=ExitCode.INCORRECT_USAGE,
    )


def test_move_unknown_platform_rejected(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_workspace(tmp_pixi_workspace, ["linux-64", "osx-64"])
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "move",
        "win-64",
        "--to-top",
        "--no-install",
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains="win-64",
    )


# ----------------------------------------------------------------------------
# list
# ----------------------------------------------------------------------------


def test_list_default_human(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(pixi, tmp_pixi_workspace, "add", "linux-64", "osx-64", "--no-install")
    # The host detection appears as a labelled "Your current machine was
    # detected as:" block, followed by a `Platforms:` header and one row
    # per workspace platform.
    out = _run_platform(
        pixi,
        tmp_pixi_workspace,
        "list",
        stdout_contains=[
            "Your current machine was detected as:",
            "Platforms:",
            "linux-64: platform=linux-64",
            "osx-64: platform=osx-64",
        ],
    )
    assert out.returncode == 0


def test_list_alias_ls(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(pixi, tmp_pixi_workspace, "add", "linux-64", "--no-install")
    _run_platform(pixi, tmp_pixi_workspace, "ls", stdout_contains=["linux-64"])


def test_list_json(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(pixi, tmp_pixi_workspace, "add", "linux-64", "osx-arm64", "--no-install")
    out = _run_platform(pixi, tmp_pixi_workspace, "list", "--json")
    payload = json.loads(out.stdout)
    # New shape: `{current_subdir, platforms: [autodetected, ...workspace]}`.
    # The auto-detected host comes first, with `is_autodetected: true`.
    assert "current_subdir" in payload
    assert payload["platforms"][0].get("is_autodetected") is True
    names = [p["name"] for p in payload["platforms"][1:]]
    assert "linux-64" in names and "osx-arm64" in names


def test_list_shows_rich_platform_packages(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "add",
        "gpu-linux=linux-64",
        "--cuda",
        "12.0",
        "--no-install",
    )
    # The one-line entry uses the same friendly key (`cuda`) as the add
    # flag rather than the raw `__cuda` form.
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "list",
        stdout_contains=["gpu-linux: platform=linux-64, cuda=12.0"],
    )


# ----------------------------------------------------------------------------
# remove
# ----------------------------------------------------------------------------


def test_remove_single(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(pixi, tmp_pixi_workspace, "add", "linux-64", "osx-64", "--no-install")
    _run_platform(pixi, tmp_pixi_workspace, "remove", "osx-64", "--no-install")
    assert "osx-64" not in _platforms_from_toml(tmp_pixi_workspace / "pixi.toml")


def test_remove_multiple(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "add",
        "linux-64",
        "osx-64",
        "win-64",
        "--no-install",
    )
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "remove",
        "osx-64",
        "win-64",
        "--no-install",
    )
    platforms = _platforms_from_toml(tmp_pixi_workspace / "pixi.toml")
    names = [p if isinstance(p, str) else p["name"] for p in platforms]
    assert "osx-64" not in names
    assert "win-64" not in names


def test_remove_alias_rm(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(pixi, tmp_pixi_workspace, "add", "linux-64", "osx-64", "--no-install")
    _run_platform(pixi, tmp_pixi_workspace, "rm", "osx-64", "--no-install")


def test_remove_unknown_rejected(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "remove",
        "no-such-platform",
        "--no-install",
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains="no-such-platform",
    )


def test_remove_custom_platform_drops_vps(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_with_rich_platform(tmp_pixi_workspace, pixi)
    _run_platform(pixi, tmp_pixi_workspace, "remove", "gpu-linux", "--no-install")
    names = [
        p if isinstance(p, str) else p["name"]
        for p in _platforms_from_toml(tmp_pixi_workspace / "pixi.toml")
    ]
    assert "gpu-linux" not in names


# ----------------------------------------------------------------------------
# list (richer scenarios)
# ----------------------------------------------------------------------------


def test_list_shows_rich_platform_in_block(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Rich entries render as `<name>: platform=..., <friendly>=<value>`."""
    _seed_with_rich_platform(tmp_pixi_workspace, pixi)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "list",
        stdout_contains=["gpu-linux: platform=linux-64, cuda=11.0"],
    )


def test_list_json_payload_shape(pixi: Path, tmp_pixi_workspace: Path) -> None:
    _seed_with_rich_platform(tmp_pixi_workspace, pixi)
    out = _run_platform(pixi, tmp_pixi_workspace, "list", "--json")
    payload = json.loads(out.stdout)
    auto = payload["platforms"][0]
    assert auto.get("is_autodetected") is True
    assert auto["name"] == "current"
    gpu = next(p for p in payload["platforms"][1:] if p["name"] == "gpu-linux")
    assert gpu["subdir"] == "linux-64"
    assert "cuda=11.0" in gpu["virtual_packages"]
    assert "detected_virtual_packages" in gpu


def test_list_with_only_current_platform(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """A workspace with only the current platform lists the host header and
    marks the matching workspace row as supported."""
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "list",
        stdout_contains=[
            "Your current machine was detected as:",
            "Platforms:",
            f"{CURRENT_PLATFORM}: platform={CURRENT_PLATFORM} (supported by current machine)",
        ],
    )


def test_list_omits_supported_marker_for_non_matching_subdir(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """A platform whose subdir doesn't match the host gets no support
    marker, and the host can't drag it along."""
    # Pick a subdir that's guaranteed not to match the current host. On
    # the off chance CI ever runs on that exact subdir, fall back to a
    # different one so the test stays meaningful.
    other = "linux-aarch64" if CURRENT_PLATFORM != "linux-aarch64" else "osx-arm64"
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(pixi, tmp_pixi_workspace, "add", other, "--no-install")
    out = _run_platform(pixi, tmp_pixi_workspace, "list")
    assert f"{other}: platform={other}" in out.stdout
    # The non-matching row never carries the support marker.
    assert f"{other}: platform={other} (supported by current machine)" not in out.stdout


def test_list_shows_environments_and_features_using_platform(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """When the manifest references a platform from features/environments,
    those names appear as indented `Used in ...` lines under the row."""
    manifest = tmp_pixi_workspace / "pixi.toml"
    manifest.write_text(
        f"""\
[workspace]
name = "platform-test"
channels = []
platforms = ["{CURRENT_PLATFORM}"]

[feature.cuda]
platforms = ["{CURRENT_PLATFORM}"]

[environments]
gpu = ["cuda"]
"""
    )
    out = _run_platform(pixi, tmp_pixi_workspace, "list")
    assert f"{CURRENT_PLATFORM}:" in out.stdout
    # Environments listing the platform: both `default` (implicit) and `gpu`.
    assert "    Used in environments: default, gpu" in out.stdout
    assert "    Used in features    : cuda" in out.stdout


def test_list_omits_usage_lines_for_single_environment_workspace(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """A workspace with only the implicit `default` environment and no
    feature platform pins emits neither usage line: the env line would
    only ever say `default`, and there are no features to mention."""
    _seed_workspace(tmp_pixi_workspace)
    out = _run_platform(pixi, tmp_pixi_workspace, "list")
    assert "Used in environments" not in out.stdout
    assert "Used in features" not in out.stdout


def test_list_respects_conda_override_cuda(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """A workspace platform pinned to `__cuda=11.0` should pick up the
    `CONDA_OVERRIDE_CUDA=12.0` env var: the host now claims `__cuda=12.0`,
    which satisfies the platform's `>=11.0` requirement, so the row is
    marked supported."""
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "add",
        f"with-cuda={CURRENT_PLATFORM}",
        "--cuda",
        "11.0",
        "--no-install",
    )
    out = verify_cli_command(
        [
            str(pixi),
            "workspace",
            "--manifest-path",
            str(tmp_pixi_workspace / "pixi.toml"),
            "platform",
            "list",
        ],
        env={"CONDA_OVERRIDE_CUDA": "12.0"},
        strip_ansi=True,
    )
    cuda_line = next(
        (line for line in out.stdout.splitlines() if line.startswith("with-cuda:")),
        None,
    )
    assert cuda_line is not None
    assert cuda_line.endswith(" (supported by current machine)"), cuda_line
    # The host header echoes the override so users can see what they're
    # being matched against.
    assert "cuda=12.0" in out.stdout


def test_list_respects_pixi_override_platform(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Setting `PIXI_OVERRIDE_PLATFORM` cross-targets the listing: a
    platform whose subdir matches the override gets the support marker
    even when it doesn't match the literal host."""
    other = "linux-aarch64" if CURRENT_PLATFORM != "linux-aarch64" else "osx-arm64"
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(pixi, tmp_pixi_workspace, "add", other, "--no-install")
    out = verify_cli_command(
        [
            str(pixi),
            "workspace",
            "--manifest-path",
            str(tmp_pixi_workspace / "pixi.toml"),
            "platform",
            "list",
        ],
        env={"PIXI_OVERRIDE_PLATFORM": other},
        strip_ansi=True,
    )
    target_line = next(
        (line for line in out.stdout.splitlines() if line.startswith(f"{other}:")),
        None,
    )
    assert target_line is not None
    assert target_line.endswith(" (supported by current machine)"), target_line
    # The host header agrees with the override.
    assert f"platform={other}" in out.stdout


def test_list_dims_unreachable_environments_and_features(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """An env/feature whose platforms are all unreachable on this host is
    dimmed in the `Used in ...` continuation lines. Reachable entries
    on the same line keep their normal styling."""
    other = "linux-aarch64" if CURRENT_PLATFORM != "linux-aarch64" else "osx-arm64"
    manifest = tmp_pixi_workspace / "pixi.toml"
    manifest.write_text(
        f"""\
[workspace]
name = "platform-test"
channels = []
platforms = ["{CURRENT_PLATFORM}", "{other}"]

[feature.only-other]
platforms = ["{other}"]

[feature.host-side]
platforms = ["{CURRENT_PLATFORM}"]

[environments]
unreachable = ["only-other"]
"""
    )
    # `--color always` forces the ANSI sequences through so we can spot
    # the dim escape (`ESC[2m`) around the unreachable names.
    out = verify_cli_command(
        [
            str(pixi),
            "--color",
            "always",
            "workspace",
            "--manifest-path",
            str(manifest),
            "platform",
            "list",
        ],
    )
    dim = "\x1b[2m"
    # `unreachable` is the environment whose only feature pins a
    # non-host subdir, so it must be dim-wrapped.
    assert f"{dim}unreachable" in out.stdout
    # `only-other` is dim because its sole platform doesn't run here;
    # `host-side` is reachable and must stay un-dimmed.
    assert f"{dim}only-other" in out.stdout
    assert f"{dim}host-side" not in out.stdout


def test_list_marks_rich_platform_unsupported_when_vps_unsatisfied(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """A rich entry on the host subdir whose customised virtual package
    the host can't satisfy is not marked as supported."""
    _seed_workspace(tmp_pixi_workspace)
    # `__cuda=999.0` is essentially guaranteed not to be present on any CI
    # host, so the row must lack the support marker.
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "add",
        f"cuda-pinned={CURRENT_PLATFORM}",
        "__cuda=999.0",
        "--no-install",
    )
    out = _run_platform(pixi, tmp_pixi_workspace, "list")
    cuda_line = next(
        (line for line in out.stdout.splitlines() if line.startswith("cuda-pinned:")),
        None,
    )
    assert cuda_line is not None
    assert not cuda_line.endswith(" (supported by current machine)")


# ----------------------------------------------------------------------------
# TOML round-trip via re-parsing after the CLI rewrites
# ----------------------------------------------------------------------------


def test_round_trip_mixed_bare_and_rich(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Mixed entries: bare entries stay as strings, rich entries stay as tables."""
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(pixi, tmp_pixi_workspace, "add", "linux-64", "--no-install")
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "add",
        "gpu-linux=linux-64",
        "--cuda",
        "12.0",
        "--no-install",
    )
    platforms = _platforms_from_toml(tmp_pixi_workspace / "pixi.toml")
    assert "linux-64" in platforms  # bare string survives as bare string
    rich = next(p for p in platforms if isinstance(p, dict) and p["name"] == "gpu-linux")
    assert rich["platform"] == "linux-64"
    assert rich["cuda"] == "12.0"


def test_round_trip_after_edit_preserves_other_entries(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    _seed_workspace(tmp_pixi_workspace)
    _run_platform(pixi, tmp_pixi_workspace, "add", "linux-64", "--no-install")
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "add",
        "gpu-linux=linux-64",
        "--cuda",
        "11.0",
        "--no-install",
    )
    _run_platform(
        pixi,
        tmp_pixi_workspace,
        "edit",
        "gpu-linux",
        "--cuda",
        "12.4",
        "--no-install",
    )
    platforms = _platforms_from_toml(tmp_pixi_workspace / "pixi.toml")
    assert "linux-64" in platforms
    rich = next(p for p in platforms if isinstance(p, dict) and p["name"] == "gpu-linux")
    assert rich["cuda"] == "12.4"


# ----------------------------------------------------------------------------
# pre-v7 lockfile lookup
#
# A v6 lockfile keys its platform rows by the bare conda subdir (`osx-arm64`)
# and records no virtual packages -- that format predates them. When the
# workspace migrates a `[system-requirements]` into a rich platform, that
# platform's name (`osx-arm64-macos-12-0`) no longer matches the lock's subdir
# key. Every command that pulls packages out of the lock for a workspace
# platform must fall back to the subdir, or it reports an empty environment on
# the affected machine. These run `--frozen` so the committed lock is read
# as-is with no solve and no network.
# ----------------------------------------------------------------------------


# Mocks an arm Mac: the host subdir plus the `__osx` the migrated platform
# requires, so `best_declared_platform` selects `osx-arm64-macos-12-0`.
_OSX_ARM64_MACHINE_ENV = {
    "PIXI_OVERRIDE_PLATFORM": "osx-arm64",
    "CONDA_OVERRIDE_OSX": "12.0",
}


def _seed_workspace_with_v6_lock(path: Path) -> Path:
    """Workspace whose `[system-requirements]` migrates `osx-arm64` into the
    rich platform `osx-arm64-macos-12-0`, plus a pre-v7 lockfile keyed by the
    bare subdir. The lock holds one conda package per platform so the
    read-only commands have something to report."""
    manifest = path / "pixi.toml"
    manifest.write_text(
        """\
[workspace]
name = "sysreq-v6"
channels = ["conda-forge"]
platforms = ["linux-64", "osx-arm64"]

[dependencies]
dummy = "*"

[system-requirements]
macos = "12.0"
"""
    )
    (path / "pixi.lock").write_text(
        """\
version: 6
environments:
  default:
    channels:
    - url: https://conda.anaconda.org/conda-forge/
    packages:
      osx-arm64:
      - conda: https://conda.anaconda.org/conda-forge/osx-arm64/dummy-1.0-h0.conda
      linux-64:
      - conda: https://conda.anaconda.org/conda-forge/linux-64/dummy-1.0-h0.conda
packages:
- conda: https://conda.anaconda.org/conda-forge/osx-arm64/dummy-1.0-h0.conda
  sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
  name: dummy
  version: '1.0'
  build: h0
  build_number: 0
  subdir: osx-arm64
  depends:
  - __osx >=11.0
  license: MIT
  size: 1234
  timestamp: 1700000000000
- conda: https://conda.anaconda.org/conda-forge/linux-64/dummy-1.0-h0.conda
  sha256: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
  name: dummy
  version: '1.0'
  build: h0
  build_number: 0
  subdir: linux-64
  license: MIT
  size: 1234
  timestamp: 1700000000000
"""
    )
    return manifest


@pytest.mark.parametrize("command", ["list", "tree"])
def test_v6_lock_resolves_for_explicit_migrated_subdir(
    pixi: Path, tmp_pixi_workspace: Path, command: str
) -> None:
    """`--platform osx-arm64-macos-12-0` names the migrated rich platform
    directly, whose packages still live under the bare `osx-arm64` key in the v6
    lock. Host-independent: the explicit platform skips the current-machine
    filter. (Passing the bare `osx-arm64` would resolve to a fresh subdir
    platform that matches the lock key directly and never exercise the rich-name
    fallback.)"""
    manifest = _seed_workspace_with_v6_lock(tmp_pixi_workspace)
    verify_cli_command(
        [
            str(pixi),
            command,
            "--frozen",
            "--platform",
            "osx-arm64-macos-12-0",
            "--manifest-path",
            str(manifest),
        ],
        stdout_contains="dummy",
        stderr_excludes="No packages found",
        strip_ansi=True,
    )


@pytest.mark.parametrize("command", ["list", "tree"])
def test_v6_lock_resolves_for_current_machine(
    pixi: Path, tmp_pixi_workspace: Path, command: str
) -> None:
    """The reported failure: on an arm Mac the environment's best platform is
    `osx-arm64-macos-12-0`, and reading the v6 lock by that name used to miss
    the subdir-keyed row, erroring with 'No packages found'."""
    manifest = _seed_workspace_with_v6_lock(tmp_pixi_workspace)
    verify_cli_command(
        [str(pixi), command, "--frozen", "--manifest-path", str(manifest)],
        env=_OSX_ARM64_MACHINE_ENV,
        stdout_contains="dummy",
        stderr_excludes="No packages found",
        strip_ansi=True,
    )


if __name__ == "__main__":  # pragma: no cover - convenience entry point
    sys.exit(pytest.main([__file__, "-x", "-q"]))
