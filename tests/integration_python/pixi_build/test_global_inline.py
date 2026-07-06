"""Integration tests for inline package definitions in `pixi global`
(`--build-backend` / `--package`), introduced in #6521.

These cover the end-to-end paths that can't be unit-tested in Rust: building a
manifest-less source through the real backend, name inference, and the
fingerprint-driven rebuild. The pure spec-conversion and manifest-overwrite
logic is covered by Rust unit tests in `pixi_cli`/`pixi_global`.

Run a single one with `pixi run test-specific-test <substring>`.
"""

from pathlib import Path

import pytest
import tomli_w
import tomllib

from .common import (
    copy_manifest,
    copytree_with_local_backend,
    exec_extension,
    git_test_repo,
    verify_cli_command,
)

# The hermetic fixture is a directory with only a `recipe.yaml` (no pixi
# package manifest); `pixi-build-rattler-build` builds it into a
# `simple-package` tool that prints "hello from simple-package".
RATTLER_BUILD = "pixi-build-rattler-build"


def _modifiable_recipe_source(build_data: Path, dest: Path) -> Path:
    """Copy the recipe-only fixture so its recipe can be edited between builds.
    `copy_manifest` bumps the mtime so a stale build cache isn't reused."""
    copytree_with_local_backend(
        build_data.joinpath("inline-package"),
        dest,
        copy_function=copy_manifest,
    )
    return dest


def _rewrite_recipe_message(source_project: Path, old: str, new: str) -> None:
    recipe_path = source_project / "recipe.yaml"
    recipe_path.write_text(recipe_path.read_text().replace(old, new))


def test_install_inline_from_git_infers_name(pixi: Path, tmp_path: Path, build_data: Path) -> None:
    """A git source with no pixi manifest builds via `--build-backend`, and the
    package name is inferred from the recipe."""
    pixi_home = tmp_path / "pixi_home"
    env = {"PIXI_HOME": str(pixi_home)}

    git_url = git_test_repo(build_data.joinpath("inline-package"), "inline-git", tmp_path)

    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--git",
            git_url,
            "--build-backend",
            RATTLER_BUILD,
        ],
        env=env,
    )

    simple_package = pixi_home / "bin" / exec_extension("simple-package")
    verify_cli_command([simple_package], env=env, stdout_contains="hello from simple-package")


@pytest.mark.slow
def test_manifest_inline_edit_triggers_rebuild(
    pixi: Path, tmp_path: Path, build_data: Path
) -> None:
    """Hand-editing the inline `package` table in the manifest changes the source
    fingerprint, so `sync` detects the env as out of sync and rebuilds. Guards the
    new behavior that source fingerprints include the inline definition."""
    pixi_home = tmp_path / "pixi_home"
    env = {"PIXI_HOME": str(pixi_home)}
    source_project = _modifiable_recipe_source(build_data, tmp_path / "src")

    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--path",
            source_project,
            "simple-package",
            "--build-backend",
            RATTLER_BUILD,
        ],
        env=env,
    )
    simple_package = pixi_home / "bin" / exec_extension("simple-package")

    # Edit the source; `sync` alone would ignore this (the spec is unchanged) ...
    _rewrite_recipe_message(
        source_project, "hello from simple-package", "goodbye from simple-package"
    )

    # ... and edit the inline definition in the manifest. This changes the
    # recorded fingerprint, which should mark the env out of sync.
    manifest_path = pixi_home.joinpath("manifests", "pixi-global.toml")
    manifest = tomllib.loads(manifest_path.read_text())
    backend = manifest["envs"]["simple-package"]["dependencies"]["simple-package"]["package"][
        "build"
    ]["backend"]
    assert "version" not in backend
    backend["version"] = "*"
    manifest_path.write_text(tomli_w.dumps(manifest))

    verify_cli_command([pixi, "global", "sync"], env=env)
    verify_cli_command([simple_package], env=env, stdout_contains="goodbye from simple-package")
