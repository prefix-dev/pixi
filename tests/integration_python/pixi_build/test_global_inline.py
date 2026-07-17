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
    CONDA_FORGE_CHANNEL,
    ExitCode,
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


def test_add_inline_to_existing_environment(
    pixi: Path, tmp_path: Path, build_data: Path, dummy_channel_1: str
) -> None:
    """`pixi global add --build-backend` records an inline definition for a
    source added to an existing environment and builds it."""
    pixi_home = tmp_path / "pixi_home"
    env = {"PIXI_HOME": str(pixi_home)}

    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            dummy_channel_1,
            "--environment",
            "test-env",
            "dummy-f",
        ],
        env=env,
    )

    verify_cli_command(
        [
            pixi,
            "global",
            "add",
            "--environment",
            "test-env",
            "--path",
            build_data.joinpath("inline-package"),
            "--build-backend",
            RATTLER_BUILD,
            "--expose",
            "simple-package=simple-package",
        ],
        env=env,
    )

    manifest_path = pixi_home.joinpath("manifests", "pixi-global.toml")
    manifest = tomllib.loads(manifest_path.read_text())
    dependency = manifest["envs"]["test-env"]["dependencies"]["simple-package"]
    assert dependency["package"]["build"]["backend"]["name"] == RATTLER_BUILD

    simple_package = pixi_home / "bin" / exec_extension("simple-package")
    verify_cli_command([simple_package], env=env, stdout_contains="hello from simple-package")


def test_missing_manifest_hints_at_build_backend(pixi: Path, tmp_path: Path) -> None:
    """Installing a named source dependency whose checkout contains no
    manifest at all fails with a hint pointing at `--build-backend`."""
    pixi_home = tmp_path / "pixi_home"
    env = {"PIXI_HOME": str(pixi_home)}
    source = tmp_path / "src"
    source.mkdir()
    source.joinpath("README.md").write_text("no manifest here")

    verify_cli_command(
        [pixi, "global", "install", "--path", source, "simple-package"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains="--build-backend",
    )


def test_package_less_manifest_hints_at_build_backend(pixi: Path, tmp_path: Path) -> None:
    """A source with a workspace-only `pixi.toml` (no `[package]` section)
    cannot be built either; the failure carries the same `--build-backend`
    hint."""
    pixi_home = tmp_path / "pixi_home"
    env = {"PIXI_HOME": str(pixi_home)}
    source = tmp_path / "src"
    source.mkdir()
    source.joinpath("pixi.toml").write_text(
        '[workspace]\nchannels = []\nplatforms = []\npreview = ["pixi-build"]\n'
    )

    verify_cli_command(
        [pixi, "global", "install", "--path", source, "simple-package"],
        ExitCode.FAILURE,
        env=env,
        stderr_contains="--build-backend",
    )


@pytest.mark.slow
def test_inference_uses_environment_channels(
    pixi: Path, tmp_path: Path, build_data: Path, dummy_channel_1: str
) -> None:
    """Name inference solves the build backend against the channels the
    environment ends up with, not the config's default channels. The defaults
    point at a dummy channel that does not carry the backend, so both commands
    below only succeed when the environment channels reach inference.

    The backend override is disabled because it bypasses the backend
    environment solve this test is about; the backend comes from conda-forge
    instead."""
    pixi_home = tmp_path / "pixi_home"
    pixi_home.mkdir()
    pixi_home.joinpath("config.toml").write_text(f'default-channels = ["{dummy_channel_1}"]\n')
    env = {"PIXI_HOME": str(pixi_home), "PIXI_BUILD_BACKEND_OVERRIDE": ""}

    # `install`: the --channel argument has to reach inference.
    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--channel",
            CONDA_FORGE_CHANNEL,
            "--path",
            build_data.joinpath("inline-package"),
            "--build-backend",
            RATTLER_BUILD,
        ],
        env=env,
    )
    simple_package = pixi_home / "bin" / exec_extension("simple-package")
    verify_cli_command([simple_package], env=env, stdout_contains="hello from simple-package")

    # `add` has no --channel flag; the channels of the target environment
    # have to reach inference.
    second_source = tmp_path / "second-src"
    copytree_with_local_backend(build_data.joinpath("inline-package"), second_source)
    recipe = second_source / "recipe.yaml"
    recipe.write_text(recipe.read_text().replace("simple-package", "second-package"))

    verify_cli_command(
        [
            pixi,
            "global",
            "add",
            "--environment",
            "simple-package",
            "--path",
            second_source,
            "--build-backend",
            RATTLER_BUILD,
        ],
        env=env,
    )

    manifest = tomllib.loads(pixi_home.joinpath("manifests", "pixi-global.toml").read_text())
    assert "second-package" in manifest["envs"]["simple-package"]["dependencies"]


def test_sync_unchanged_inline_env_is_noop(pixi: Path, tmp_path: Path, build_data: Path) -> None:
    """`pixi global sync` leaves an unchanged source environment alone. The
    fingerprints recorded at install time must match the ones recomputed from
    the manifest; if they ever diverge for an unchanged manifest, every sync
    rebuilds every source environment."""
    pixi_home = tmp_path / "pixi_home"
    env = {"PIXI_HOME": str(pixi_home)}

    verify_cli_command(
        [
            pixi,
            "global",
            "install",
            "--path",
            build_data.joinpath("inline-package"),
            "--build-backend",
            RATTLER_BUILD,
        ],
        env=env,
    )

    # The environment file is rewritten whenever the environment is
    # (re)installed, so a stable mtime means the sync was a no-op.
    env_file = pixi_home.joinpath("envs", "simple-package", "conda-meta", "pixi")
    mtime_before = env_file.stat().st_mtime_ns

    verify_cli_command([pixi, "global", "sync"], env=env)

    assert env_file.stat().st_mtime_ns == mtime_before, (
        "sync rebuilt an unchanged source environment"
    )
