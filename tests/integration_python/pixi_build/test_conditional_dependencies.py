"""Tests for `if(...)` conditional package dependencies in the `[package]` section."""

from pathlib import Path
from typing import Any

import tomli_w
from rattler.lock import LockFile

from .common import ExitCode, verify_cli_command

# The platforms covered by target_specific_channel_1. The channel only ships
# `package-unix` for the unix platforms and `package-windows` for win-64, so a
# conditional dependency that leaks to the wrong platform fails the solve.
PLATFORMS = ["linux-64", "osx-64", "osx-arm64", "win-64"]
UNIX_PLATFORMS = ["linux-64", "osx-64", "osx-arm64"]

PACKAGE_NAME = "conditional-deps"


def write_workspace(
    workspace_dir: Path,
    channel: str,
    run_dependencies: dict[str, Any],
    platforms: list[str] = PLATFORMS,
    build_variants: dict[str, list[str]] | None = None,
) -> Path:
    """Write a workspace manifest with a source package using `pixi-build-cmake`."""
    workspace: dict[str, Any] = {
        # The channel also contains stub `cmake` and `ninja` packages for
        # the build environment of the backend, locking stays offline.
        "channels": [channel],
        "platforms": platforms,
        "preview": ["pixi-build"],
    }
    if build_variants is not None:
        workspace["build-variants"] = build_variants
    manifest: dict[str, Any] = {
        "workspace": workspace,
        "dependencies": {PACKAGE_NAME: {"path": "."}},
        "package": {
            "name": PACKAGE_NAME,
            "version": "1.0.0",
            "build": {
                "backend": {"name": "pixi-build-cmake", "version": "*"},
                # No compilers keeps the build environment small.
                "config": {"compilers": []},
            },
            "run-dependencies": run_dependencies,
        },
    }
    manifest_path = workspace_dir.joinpath("pixi.toml")
    manifest_path.write_text(tomli_w.dumps(manifest))
    return manifest_path


def locked_package_names(workspace_dir: Path) -> dict[str, set[str]]:
    """Map each platform in the default environment to its locked package names."""
    lock = LockFile.from_path(workspace_dir.joinpath("pixi.lock"))
    environment = lock.default_environment()
    assert environment is not None
    return {
        str(platform): {package.name for package in packages}
        for platform, packages in environment.packages_by_platform().items()
    }


def test_simple_conditional_dependency(
    pixi: Path, tmp_pixi_workspace: Path, target_specific_channel_1: str
) -> None:
    """A platform family condition only adds the dependency on matching platforms."""
    manifest_path = write_workspace(
        tmp_pixi_workspace,
        target_specific_channel_1,
        {
            "if(unix)": {"package-unix": "*"},
            "if(win)": {"package-windows": "*"},
        },
    )

    verify_cli_command([pixi, "lock", "--manifest-path", manifest_path])

    packages = locked_package_names(tmp_pixi_workspace)
    for platform in UNIX_PLATFORMS:
        assert "package-unix" in packages[platform]
        assert "package-windows" not in packages[platform]
    assert "package-windows" in packages["win-64"]
    assert "package-unix" not in packages["win-64"]


def test_complex_conditional_expression(
    pixi: Path, tmp_pixi_workspace: Path, target_specific_channel_1: str
) -> None:
    """Boolean operators and platform comparisons evaluate per platform."""
    manifest_path = write_workspace(
        tmp_pixi_workspace,
        target_specific_channel_1,
        {"if(unix and not (host_platform == 'osx-arm64'))": {"package-unix": "*"}},
    )

    verify_cli_command([pixi, "lock", "--manifest-path", manifest_path])

    packages = locked_package_names(tmp_pixi_workspace)
    assert "package-unix" in packages["linux-64"]
    assert "package-unix" in packages["osx-64"]
    assert "package-unix" not in packages["osx-arm64"]
    assert "package-unix" not in packages["win-64"]


def test_variant_conditional_expression(
    pixi: Path, tmp_pixi_workspace: Path, target_specific_channel_1: str
) -> None:
    """`match()` conditions evaluate against the configured build variants."""
    manifest_path = write_workspace(
        tmp_pixi_workspace,
        target_specific_channel_1,
        {
            "if(match(python, '>=3.10'))": {"package-unix": "*"},
            # `package-windows` does not exist on linux-64, so the solve would
            # fail loudly if this condition were wrongly applied.
            "if(match(python, '<3.10'))": {"package-windows": "*"},
        },
        platforms=["linux-64"],
        build_variants={"python": ["3.12"]},
    )

    verify_cli_command([pixi, "lock", "--manifest-path", manifest_path])

    packages = locked_package_names(tmp_pixi_workspace)
    assert "package-unix" in packages["linux-64"]
    assert "package-windows" not in packages["linux-64"]


def test_invalid_conditional_expression(
    pixi: Path, tmp_pixi_workspace: Path, target_specific_channel_1: str
) -> None:
    """An expression that does not parse fails the lock with a helpful error."""
    manifest_path = write_workspace(
        tmp_pixi_workspace,
        target_specific_channel_1,
        {"if(host_platform ==)": {"package-unix": "*"}},
        # A single fixed platform keeps the error output deterministic.
        platforms=["linux-64"],
    )

    verify_cli_command(
        [pixi, "lock", "--manifest-path", manifest_path],
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains="invalid selector expression `host_platform ==`",
    )
