"""Integration tests for inline package definitions.

An inline package definition lets a source dependency carry its own ``[package]``
table directly on the dependency spec, so the referenced source needs no on-disk
``pixi.toml``::

    [dependencies]
    rust-app = { path = "pkg", package = { build = { backend = { name = "pixi-build-rust" } } } }

These tests deliberately build real packages (and one clones a local git repo),
so they are slow and exercise the network. They are NOT meant to be part of the
fast default selection; the build cases are marked ``slow``.

Run them with::

    pixi run test-specific-test inline_package        # release backends
    pixi run test-specific-test-debug inline_package  # debug backends
"""

from pathlib import Path

import pytest
import tomli_w

from .common import (
    CONDA_FORGE_CHANNEL,
    CURRENT_PLATFORM,
    ExitCode,
    git_test_repo,
    verify_cli_command,
)

# Channels a build backend is resolved from. The session-autouse
# ``setup_build_backend_override`` fixture redirects the actual backend binaries
# to the locally built ones, but the channels still need to be declared.
BACKEND_CHANNELS = [
    "https://prefix.dev/pixi-build-backends",
    CONDA_FORGE_CHANNEL,
]

# Emitted on stderr (with `-v`) whenever a source package is actually built
# rather than served from cache. Same marker the neighbouring build tests use.
BUILD_RUNNING_STRING = "Running build for recipe:"


# --------------------------------------------------------------------------- #
# Source templates (no pixi.toml -- the [package] table lives inline instead)
# --------------------------------------------------------------------------- #

# rattler-build: a bare recipe.yaml that installs an executable printing a
# recognizable line. Mirrors tests/data/pixi-build/simple-package.
RECIPE_YAML = """\
package:
  name: simple-package
  version: 0.1.0

build:
  number: 0
  script:
    - if: win
      then:
        - if not exist "%PREFIX%\\bin" mkdir "%PREFIX%\\bin"
        - echo @echo off > %PREFIX%\\bin\\simple-package.bat
        - echo echo hello from inline simple-package >> %PREFIX%\\bin\\simple-package.bat
      else:
        - mkdir -p $PREFIX/bin
        - echo "#!/usr/bin/env bash" > $PREFIX/bin/simple-package
        - echo "echo hello from inline simple-package" >> $PREFIX/bin/simple-package
        - chmod +x $PREFIX/bin/simple-package
"""
RECIPE_OUTPUT = "hello from inline simple-package"

# python: a hatchling project (pyproject.toml, no [tool.pixi]). Mirrors
# tests/data/pixi-build/rich_example.
PYTHON_PYPROJECT = """\
[project]
dependencies = ["rich"]
name = "rich_example"
requires-python = ">= 3.11"
scripts = { rich-example-main = "rich_example:main" }
version = "0.1.0"

[build-system]
build-backend = "hatchling.build"
requires = ["hatchling"]
"""
PYTHON_INIT = '''\
from rich.console import Console


def main() -> None:
    Console().print("inline python package works")
'''
PYTHON_OUTPUT = "inline python package works"

# rust: a Cargo project (Cargo.toml + src/main.rs, no pixi.toml). Mirrors
# tests/data/pixi-build/minimal-backend-workspaces/pixi-build-rust.
RUST_CARGO = """\
[package]
edition = "2024"
name = "rust-app"
version = "0.1.0"

[dependencies]
"""
RUST_MAIN = """\
fn main() {
    println!("inline rust package works");
}
"""
RUST_OUTPUT = "inline rust package works"


# --------------------------------------------------------------------------- #
# Helpers
# --------------------------------------------------------------------------- #


def write_recipe_source(directory: Path) -> None:
    directory.mkdir(parents=True, exist_ok=True)
    directory.joinpath("recipe.yaml").write_text(RECIPE_YAML)


def write_python_source(directory: Path) -> None:
    pkg = directory.joinpath("src", "rich_example")
    pkg.mkdir(parents=True, exist_ok=True)
    directory.joinpath("pyproject.toml").write_text(PYTHON_PYPROJECT)
    pkg.joinpath("__init__.py").write_text(PYTHON_INIT)


def write_rust_source(directory: Path) -> None:
    src = directory.joinpath("src")
    src.mkdir(parents=True, exist_ok=True)
    directory.joinpath("Cargo.toml").write_text(RUST_CARGO)
    src.joinpath("main.rs").write_text(RUST_MAIN)


def write_consumer_manifest(
    manifest_path: Path,
    dependencies: dict,
    tasks: dict | None = None,
) -> None:
    """Write a workspace pixi.toml that declares `dependencies`."""
    manifest: dict = {
        "workspace": {
            "channels": [CONDA_FORGE_CHANNEL],
            "platforms": [CURRENT_PLATFORM],
            "preview": ["pixi-build"],
        },
        "dependencies": dependencies,
    }
    if tasks:
        manifest["tasks"] = tasks
    manifest_path.write_text(tomli_w.dumps(manifest))


def python_inline_package() -> dict:
    return {
        "build": {
            "backend": {
                "name": "pixi-build-python",
                "version": "*",
                "channels": BACKEND_CHANNELS,
            }
        },
        "host-dependencies": {"hatchling": "==1.26.3"},
        "run-dependencies": {"rich": ">=13.9.4,<14"},
    }


def rust_inline_package() -> dict:
    return {
        "build": {
            "backend": {
                "name": "pixi-build-rust",
                "version": "*",
                "channels": BACKEND_CHANNELS,
            }
        }
    }


def rattler_inline_package() -> dict:
    return {
        "build": {
            "backend": {
                "name": "pixi-build-rattler-build",
                "version": "*",
            }
        }
    }


# --------------------------------------------------------------------------- #
# Error rules (fast: these fail during manifest parsing, before any build)
# --------------------------------------------------------------------------- #


def test_inline_requires_source_location(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """An inline package with no git/path/url source location is rejected.

    A `version` selector is included so the leftover spec is an otherwise-valid
    binary spec; that is what makes the inline-specific check fire (a fully bare
    spec hits the generic "must be specified" error first).
    """
    manifest = tmp_pixi_workspace / "pixi.toml"
    write_consumer_manifest(
        manifest,
        {"foo": {"version": "==1.0.0", "package": rust_inline_package()}},
    )
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains="an inline package definition requires a `git`, `path` or `url` source location",
    )


def test_inline_cannot_set_name(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """`name` is taken from the dependency key and cannot be set inline."""
    package = rust_inline_package()
    package["name"] = "foo"
    manifest = tmp_pixi_workspace / "pixi.toml"
    write_consumer_manifest(manifest, {"foo": {"path": "pkg", "package": package}})
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains="an inline package definition cannot set `name`",
    )


def test_inline_cannot_set_build_source(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """The source comes from the dependency spec, not `package.build.source`."""
    package = rust_inline_package()
    package["build"]["source"] = {"path": "elsewhere"}
    manifest = tmp_pixi_workspace / "pixi.toml"
    write_consumer_manifest(
        manifest,
        {"foo": {"git": "https://example.invalid/foo.git", "package": package}},
    )
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains="an inline package definition cannot set `build.source`",
    )


# --------------------------------------------------------------------------- #
# Functional builds: build the inline-defined package and run its artifact
# --------------------------------------------------------------------------- #


@pytest.mark.slow
def test_inline_overrides_ondisk_recipe(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """The inline definition must take precedence over an on-disk recipe.yaml.

    The source ships a valid recipe.yaml, but the inline package names a backend
    that cannot exist. If inline definitions are honoured (skipping on-disk
    discovery as designed), resolving the bogus backend must fail. If the inline
    def is ignored, on-disk discovery silently builds via the real rattler-build
    backend and the command wrongly succeeds -- which is exactly the dead-binding
    bug this test guards against.

    This is the discriminating counterpart to a plain "build via recipe.yaml"
    test: such a test passes whether or not the inline path fires (the
    rattler-build backend reads recipe.yaml either way), so it cannot tell a
    working feature apart from a completely absent one. This test can.
    """
    write_recipe_source(tmp_pixi_workspace / "pkg")
    package = {"build": {"backend": {"name": "pixi-build-does-not-exist"}}}
    manifest = tmp_pixi_workspace / "pixi.toml"
    write_consumer_manifest(
        manifest,
        {"simple-package": {"path": "pkg", "package": package}},
    )

    verify_cli_command(
        [pixi, "install", "-v", "--manifest-path", manifest],
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains="pixi-build-does-not-exist",
    )


@pytest.mark.slow
def test_path_points_directly_at_recipe(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """`path` may point straight at a recipe.yaml file, not just its directory.

    This exercises path-to-file source resolution, NOT the inline mechanism: the
    rattler-build backend reads recipe.yaml regardless of whether the inline
    definition is consulted, so this passes even when the inline binding is dead.
    It is kept as a regression guard for the path-handling behaviour only; see
    `test_inline_overrides_ondisk_recipe` for the inline-discriminating case.
    """
    write_recipe_source(tmp_pixi_workspace / "pkg")
    manifest = tmp_pixi_workspace / "pixi.toml"
    write_consumer_manifest(
        manifest,
        {
            "simple-package": {
                "path": "pkg/recipe.yaml",
                "package": rattler_inline_package(),
            }
        },
        tasks={"start": "simple-package"},
    )

    verify_cli_command([pixi, "install", "-v", "--manifest-path", manifest])
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "start"],
        stdout_contains=RECIPE_OUTPUT,
    )


@pytest.mark.slow
def test_inline_python_consumer_pixi_toml(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Build a pixi-build-python package, declared from a pixi.toml workspace."""
    write_python_source(tmp_pixi_workspace / "pkg")
    manifest = tmp_pixi_workspace / "pixi.toml"
    write_consumer_manifest(
        manifest,
        {"rich_example": {"path": "pkg", "package": python_inline_package()}},
        tasks={"start": "rich-example-main"},
    )

    verify_cli_command([pixi, "install", "-v", "--manifest-path", manifest])
    # The inline run-dependency must surface in the solved lock, proving the
    # inline package manifest (not an auto-discovered one) drove the solve.
    assert "rich" in (tmp_pixi_workspace / "pixi.lock").read_text()
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "start"],
        stdout_contains=PYTHON_OUTPUT,
    )


@pytest.mark.slow
def test_inline_python_consumer_pyproject(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Same package, but the consuming workspace is a pyproject.toml [tool.pixi].

    Exercises the second consumer-manifest format: the inline dependency is
    declared under [tool.pixi.dependencies] instead of a pixi.toml.
    """
    write_python_source(tmp_pixi_workspace / "pkg")
    backend_channels = ", ".join(f'"{c}"' for c in BACKEND_CHANNELS)
    pyproject = tmp_pixi_workspace / "pyproject.toml"
    pyproject.write_text(
        f"""\
[tool.pixi.workspace]
channels = ["{CONDA_FORGE_CHANNEL}"]
platforms = ["{CURRENT_PLATFORM}"]
preview = ["pixi-build"]

[tool.pixi.tasks]
start = "rich-example-main"

[tool.pixi.dependencies]
rich_example = {{ path = "pkg", package = {{ build = {{ backend = {{ name = "pixi-build-python", version = "*", channels = [{backend_channels}] }} }}, host-dependencies = {{ hatchling = "==1.26.3" }}, run-dependencies = {{ rich = ">=13.9.4,<14" }} }} }}
"""
    )

    verify_cli_command([pixi, "install", "-v", "--manifest-path", pyproject])
    # The inline run-dependency must surface in the solved lock, proving the
    # inline package manifest (not an auto-discovered one) drove the solve.
    assert "rich" in (tmp_pixi_workspace / "pixi.lock").read_text()
    verify_cli_command(
        [pixi, "run", "--manifest-path", pyproject, "start"],
        stdout_contains=PYTHON_OUTPUT,
    )


@pytest.mark.slow
def test_inline_rust_path(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Build a pixi-build-rust package from a bare Cargo project and run it."""
    write_rust_source(tmp_pixi_workspace / "pkg")
    manifest = tmp_pixi_workspace / "pixi.toml"
    write_consumer_manifest(
        manifest,
        {"rust-app": {"path": "pkg", "package": rust_inline_package()}},
        tasks={"start": "rust-app"},
    )

    verify_cli_command([pixi, "install", "-v", "--manifest-path", manifest])
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "start"],
        stdout_contains=RUST_OUTPUT,
    )


@pytest.mark.slow
def test_inline_git_source(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """An inline package whose source is a git repository (no pixi.toml in it)."""
    source_template = tmp_pixi_workspace / "_rust_template"
    write_rust_source(source_template)
    git_uri = git_test_repo(source_template, "rust-git-repo", tmp_pixi_workspace)

    manifest = tmp_pixi_workspace / "pixi.toml"
    write_consumer_manifest(
        manifest,
        {"rust-app": {"git": git_uri, "package": rust_inline_package()}},
        tasks={"start": "rust-app"},
    )

    verify_cli_command([pixi, "install", "-v", "--manifest-path", manifest])

    # The pinned git source is recorded in the lock file.
    lock = (tmp_pixi_workspace / "pixi.lock").read_text()
    assert f"git+{git_uri}" in lock

    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "start"],
        stdout_contains=RUST_OUTPUT,
    )


# --------------------------------------------------------------------------- #
# Cache invalidation
# --------------------------------------------------------------------------- #


@pytest.mark.slow
def test_inline_change_triggers_rebuild(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Editing the inline `package` table must invalidate the build cache.

    This encodes the semantics of the design: the inline package content
    participates in cache invalidation, so editing it forces a rebuild even
    though the source files on disk are untouched. The inline content hash folds
    into the source record's identifier (a lock input) and into the artifact
    cache key, so a changed table re-locks and rebuilds.

    A pixi-build-python package is used because its run-dependencies flow into
    the backend's outputs, so editing them is both valid and observable (unlike
    pixi-build-rattler-build, whose outputs come solely from recipe.yaml).
    """
    write_python_source(tmp_pixi_workspace / "pkg")
    manifest = tmp_pixi_workspace / "pixi.toml"

    def install(*, expect_build: bool) -> None:
        output = verify_cli_command([pixi, "install", "-v", "--manifest-path", manifest])
        built = BUILD_RUNNING_STRING in output.stderr
        assert built == expect_build, (
            f"expected build={expect_build}, but build={built}\nstderr:\n{output.stderr}"
        )

    # First install builds the package.
    write_consumer_manifest(
        manifest,
        {"rich_example": {"path": "pkg", "package": python_inline_package()}},
    )
    install(expect_build=True)

    # No change -> fully cached, no rebuild.
    install(expect_build=False)

    # Change only the inline package table (widen the rich run-dependency); the
    # source files on disk are untouched. This must still trigger a rebuild.
    package = python_inline_package()
    package["run-dependencies"] = {"rich": ">=13.9.4,<15"}
    write_consumer_manifest(
        manifest,
        {"rich_example": {"path": "pkg", "package": package}},
    )
    install(expect_build=True)
