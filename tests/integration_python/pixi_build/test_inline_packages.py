"""Integration test for inline package definitions overriding on-disk discovery.

An inline package definition lets a source dependency carry its own ``[package]``
table directly on the dependency spec, so the referenced source needs no on-disk
``pixi.toml``::

    [dependencies]
    rust-app = { path = "pkg", package = { build = { backend = { name = "pixi-build-rust" } } } }

Parse validation and the content-hash behaviour of inline definitions are covered
by the ``pixi_manifest`` unit tests. What only a real run can prove is the
*threading*: that an inline definition parsed from the manifest actually survives
the whole build pipeline and reaches backend discovery, suppressing the on-disk
recipe. That is what this test guards; the heavier build-and-run cases live in a
separate, unmerged test module.

Run it with::

    pixi run test-specific-test inline_overrides        # release backends
    pixi run test-specific-test-debug inline_overrides  # debug backends
"""

import json
from pathlib import Path
from typing import Any

import pytest
import tomli_w

from .common import (
    CONDA_FORGE_CHANNEL,
    CURRENT_PLATFORM,
    ExitCode,
    git_test_repo,
    verify_cli_command,
)

# rattler-build: a bare recipe.yaml that installs an executable. Mirrors
# tests/data/pixi-build/simple-package.
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


def write_recipe_source(directory: Path) -> None:
    directory.mkdir(parents=True, exist_ok=True)
    directory.joinpath("recipe.yaml").write_text(RECIPE_YAML)


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
def test_inline_overrides_ondisk_recipe_pyproject(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Inline definitions must thread through the build pipeline identically when
    the consumer manifest is a `pyproject.toml` with a `[tool.pixi]` table.

    A `pyproject.toml` carrying `[tool.pixi]` is a first-class Pixi manifest, so
    declaring an inline package there must work exactly as in a `pixi.toml`. This
    reuses the discriminating setup of `test_inline_overrides_ondisk_recipe`: the
    source ships a valid recipe.yaml, but the inline package names a backend that
    cannot exist. If the inline definition declared under `[tool.pixi]` is
    honoured, resolving the bogus backend must fail; if it is dropped on the way
    out of the pyproject parser, on-disk discovery silently builds via the real
    rattler-build backend and the command wrongly succeeds.
    """
    write_recipe_source(tmp_pixi_workspace / "pkg")
    package = {"build": {"backend": {"name": "pixi-build-does-not-exist"}}}
    manifest = tmp_pixi_workspace / "pyproject.toml"
    manifest.write_text(
        tomli_w.dumps(
            {
                "project": {
                    "name": "consumer",
                    "version": "0.1.0",
                    "requires-python": ">=3.11",
                },
                "tool": {
                    "pixi": {
                        "workspace": {
                            "channels": [CONDA_FORGE_CHANNEL],
                            "platforms": [CURRENT_PLATFORM],
                            "preview": ["pixi-build"],
                        },
                        "dependencies": {
                            "simple-package": {"path": "pkg", "package": package},
                        },
                    }
                },
            }
        )
    )

    verify_cli_command(
        [pixi, "install", "-v", "--manifest-path", manifest],
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains="pixi-build-does-not-exist",
    )


def test_inline_definition_inherits_workspace_version(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """An inline package must be able to inherit package metadata from the
    consuming workspace, just like an on-disk `[package]`.

    The workspace here defines a version (`9.9.9`), and the inline package
    requests it with `version = { workspace = true }`. Loading the manifest must
    succeed and resolve the version from the workspace. Today it fails with
    "the workspace does not define a 'version'" because inline definitions are
    converted with an empty `WorkspacePackageProperties` instead of the consuming
    workspace's, so there is no package-to-workspace relationship to inherit from.

    `workspace environment list` only loads and converts the manifest (no solve
    or build), so this stays a fast, offline regression guard.
    """
    write_recipe_source(tmp_pixi_workspace / "pkg")
    manifest = tmp_pixi_workspace / "pyproject.toml"
    manifest.write_text(
        tomli_w.dumps(
            {
                "project": {
                    "name": "consumer",
                    "version": "9.9.9",
                    "requires-python": ">=3.11",
                },
                "tool": {
                    "pixi": {
                        "workspace": {
                            "channels": [CONDA_FORGE_CHANNEL],
                            "platforms": [CURRENT_PLATFORM],
                            "preview": ["pixi-build"],
                        },
                        "dependencies": {
                            "simple-package": {
                                "path": "pkg",
                                "package": {
                                    "version": {"workspace": True},
                                    "build": {
                                        "backend": {
                                            "name": "pixi-build-rattler-build",
                                            "version": "*",
                                        }
                                    },
                                },
                            }
                        },
                    }
                },
            }
        )
    )

    verify_cli_command(
        [pixi, "workspace", "environment", "list", "--manifest-path", manifest],
        expected_exit_code=ExitCode.SUCCESS,
        stderr_excludes="does not define a 'version'",
    )


@pytest.mark.slow
@pytest.mark.parametrize("source_kind", ["path", "git"])
def test_inline_definition_keeps_host_dependencies(
    pixi: Path, tmp_pixi_workspace: Path, source_kind: str
) -> None:
    """Regression test for https://github.com/prefix-dev/pixi/issues/6527:
    `host-dependencies` declared in an inline package definition must reach the
    build backend.

    The consumer declares an inline package with `host-dependencies =
    { setuptools = "*" }` on a path or git source. `pixi lock` only computes
    source metadata (no build, no host environment solve), and the debug build
    of the backend logs the project model it received to `<work dir>/debug/
    project_model.json`. If the inline definition's host dependencies are
    dropped anywhere between manifest parsing and backend discovery, the logged
    project model has no `setuptools` entry.
    """
    source = tmp_pixi_workspace / "pkg"
    source.mkdir(parents=True, exist_ok=True)
    source.joinpath("pyproject.toml").write_text(
        tomli_w.dumps(
            {
                "project": {"name": "inline-host-deps", "version": "0.1.0"},
                "build-system": {
                    "requires": ["setuptools"],
                    "build-backend": "setuptools.build_meta",
                },
            }
        )
    )
    if source_kind == "git":
        location = {"git": git_test_repo(source, "inline-host-deps-repo", tmp_pixi_workspace)}
    else:
        location = {"path": "pkg"}
    manifest = tmp_pixi_workspace / "pixi.toml"
    write_consumer_manifest(
        manifest,
        {
            "inline-host-deps": {
                **location,
                "package": {
                    "version": "0.1.0",
                    "build": {"backend": {"name": "pixi-build-python"}},
                    "host-dependencies": {"setuptools": "*"},
                },
            }
        },
    )

    verify_cli_command([pixi, "lock", "--manifest-path", manifest])

    # The backend metadata work directory (and with it the backend's debug
    # output) lives under the workspace's `.pixi` directory.
    project_models = list(tmp_pixi_workspace.glob("**/debug/project_model.json"))
    assert project_models, "the backend should have logged the project model it received"
    combined = "".join(path.read_text() for path in project_models)
    assert "setuptools" in combined, (
        "setuptools from the inline `host-dependencies` never reached the build backend"
    )

    # The actual build must install setuptools into the host prefix.
    verify_cli_command([pixi, "install", "--manifest-path", manifest])
    build_params = list(tmp_pixi_workspace.glob("**/debug/conda_build_v1_params.json"))
    assert build_params, "the backend should have logged the build parameters it received"
    host_packages = [
        package["name"]
        for path in build_params
        for package in json.loads(path.read_text()).get("hostPrefix", {}).get("packages", [])
    ]
    assert "setuptools" in host_packages, (
        f"setuptools from the inline `host-dependencies` is missing from the "
        f"build host environment, got: {host_packages}"
    )


@pytest.mark.slow
def test_inline_definition_edit_invalidates_lock(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Regression test for https://github.com/prefix-dev/pixi/issues/6527: editing
    an inline package definition must invalidate the lock file.

    A git source is content-pinned, so satisfiability normally trusts the
    locked record without contacting the build backend. The inline package
    definition lives in the consuming manifest, though: adding
    `run-dependencies` (or `host-dependencies`) there changes what the
    dependency resolves to without any lock-file-visible signal. Before the
    fix, a re-lock after such an edit reported "already up-to-date" and the
    new dependencies never appeared.
    """
    source = tmp_pixi_workspace / "pkg"
    source.mkdir(parents=True, exist_ok=True)
    source.joinpath("pyproject.toml").write_text(
        tomli_w.dumps(
            {
                "project": {"name": "inline-edit", "version": "0.1.0"},
                "build-system": {
                    "requires": ["setuptools"],
                    "build-backend": "setuptools.build_meta",
                },
            }
        )
    )
    repo_url = git_test_repo(source, "inline-edit-repo", tmp_pixi_workspace)
    manifest = tmp_pixi_workspace / "pixi.toml"

    def write_manifest(package: dict) -> None:
        write_consumer_manifest(
            manifest,
            {"inline-edit": {"git": repo_url, "package": package}},
        )

    package: dict[str, Any] = {
        "version": "0.1.0",
        "build": {"backend": {"name": "pixi-build-python"}},
    }
    write_manifest(package)
    verify_cli_command([pixi, "lock", "--manifest-path", manifest])
    lock_file = tmp_pixi_workspace / "pixi.lock"
    assert "rich" not in lock_file.read_text()

    # Extend the inline definition; the lock file must be re-resolved.
    package["run-dependencies"] = {"rich": "*"}
    write_manifest(package)
    verify_cli_command([pixi, "lock", "--manifest-path", manifest])
    assert "rich" in lock_file.read_text(), (
        "editing the inline package definition did not invalidate the lock file"
    )

    # An unchanged manifest must still satisfy the lock file.
    verify_cli_command(
        [pixi, "lock", "--manifest-path", manifest],
        stderr_contains="already up-to-date",
    )

    # Removing the inline definition's extra dependencies must invalidate again.
    del package["run-dependencies"]
    write_manifest(package)
    verify_cli_command([pixi, "lock", "--manifest-path", manifest])
    assert "rich" not in lock_file.read_text(), (
        "reverting the inline package definition did not invalidate the lock file"
    )


@pytest.mark.slow
def test_lower_priority_inline_does_not_leak_onto_plain_dependency(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """An inline definition must only apply when it belongs to the feature that
    wins the dependency. A lower-priority feature's inline definition must not
    attach to a higher-priority feature's plain (non-inline) source dependency of
    the same name.

    Feature `high` (listed first, higher priority) declares `simple-package` as a
    plain source dependency with no inline definition, so it must build via
    on-disk discovery -- the source ships its own `pixi.toml` naming
    `highbogusbackend`. Feature `low` declares the same package with an inline
    definition naming `lowbogusbackend`. Because the winning feature carries no
    inline definition, resolution must reach `highbogusbackend`.

    `combined_inline_packages` resolves inline definitions per feature priority:
    the highest-priority feature that declares the dependency decides whether it
    carries an inline definition, so `high`'s plain declaration suppresses
    `low`'s inline definition and resolution reaches `highbogusbackend`. A single
    environment is used so the surfaced backend is unambiguous (no
    cross-environment error ordering).
    """
    source = tmp_pixi_workspace / "pkg"
    source.mkdir(parents=True, exist_ok=True)
    source.joinpath("pixi.toml").write_text(
        tomli_w.dumps(
            {
                "workspace": {
                    "channels": [CONDA_FORGE_CHANNEL],
                    "platforms": [CURRENT_PLATFORM],
                    "preview": ["pixi-build"],
                },
                "package": {
                    "name": "simple-package",
                    "version": "0.1.0",
                    "build": {"backend": {"name": "highbogusbackend"}},
                },
            }
        )
    )
    manifest = tmp_pixi_workspace / "pixi.toml"
    manifest.write_text(
        tomli_w.dumps(
            {
                "workspace": {
                    "channels": [CONDA_FORGE_CHANNEL],
                    "platforms": [CURRENT_PLATFORM],
                    "preview": ["pixi-build"],
                },
                "feature": {
                    "high": {"dependencies": {"simple-package": {"path": "pkg"}}},
                    "low": {
                        "dependencies": {
                            "simple-package": {
                                "path": "pkg",
                                "package": {"build": {"backend": {"name": "lowbogusbackend"}}},
                            }
                        }
                    },
                },
                "environments": {"env": ["high", "low"]},
            }
        )
    )

    verify_cli_command(
        [pixi, "install", "-v", "--manifest-path", manifest, "--environment", "env"],
        expected_exit_code=ExitCode.FAILURE,
        stderr_contains="highbogusbackend",
        stderr_excludes="lowbogusbackend",
    )
