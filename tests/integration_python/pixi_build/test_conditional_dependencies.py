"""Tests for `if(...)` conditional package dependencies in the `[package]` section."""

from pathlib import Path
from typing import Any

import tomli_w
from rattler.lock import LockFile

from .common import CURRENT_PLATFORM, ExitCode, verify_cli_command

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
    manifest: dict[str, Any] = {
        "workspace": {
            # The channel also contains stub `cmake` and `ninja` packages for
            # the build environment of the backend, locking stays offline.
            "channels": [channel],
            "platforms": platforms,
            "preview": ["pixi-build"],
        },
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
    if build_variants is not None:
        manifest["workspace"]["build-variants"] = build_variants
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


def test_old_style_target_host_dependency_reaches_build_script(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """A pip host dependency declared via `[package.target.<platform>]` must be
    visible to the python backend when it picks the installer for the build
    script. Target tables are lowered to conditional expressions at parse time,
    so the backend only sees the dependency after rendering the recipe."""
    manifest: dict[str, Any] = {
        "workspace": {
            "channels": ["https://prefix.dev/conda-forge"],
            "platforms": [CURRENT_PLATFORM],
            "preview": ["pixi-build"],
        },
        "dependencies": {"conditional-installer": {"path": "."}},
        "package": {
            "name": "conditional-installer",
            "version": "1.0.0",
            "build": {"backend": {"name": "pixi-build-python", "version": "*"}},
            "host-dependencies": {"hatchling": "*"},
            "target": {CURRENT_PLATFORM: {"host-dependencies": {"pip": "*"}}},
        },
    }
    manifest_path = tmp_pixi_workspace.joinpath("pixi.toml")
    manifest_path.write_text(tomli_w.dumps(manifest))
    tmp_pixi_workspace.joinpath("pyproject.toml").write_text(
        "\n".join(
            [
                "[project]",
                'name = "conditional-installer"',
                'version = "1.0.0"',
                "",
                "[build-system]",
                'requires = ["hatchling"]',
                'build-backend = "hatchling.build"',
            ]
        )
    )
    package_dir = tmp_pixi_workspace.joinpath("src", "conditional_installer")
    package_dir.mkdir(parents=True)
    package_dir.joinpath("__init__.py").write_text("")

    verify_cli_command([pixi, "install", "--manifest-path", manifest_path])

    # The build is kept in the workspace cache, including the build script
    # that was actually executed.
    build_scripts = [
        script
        for pattern in ("**/conda_build.sh", "**/conda_build.bat")
        for script in tmp_pixi_workspace.joinpath(".pixi").glob(pattern)
    ]
    assert build_scripts, "the build should leave the executed build script in the work directory"
    for build_script in build_scripts:
        content = build_script.read_text()
        assert "-m pip install" in content, (
            f"the build script should install with the conditionally declared pip, got:\n{content}"
        )
        assert "uv pip install" not in content, (
            f"the injected uv installer must not shadow the user-declared pip, got:\n{content}"
        )
