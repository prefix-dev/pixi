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

from pathlib import Path

import pytest
import tomli_w

from .common import (
    CONDA_FORGE_CHANNEL,
    CURRENT_PLATFORM,
    ExitCode,
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

    Today inline definitions are merged across all of an environment's features
    by package name (`combined_inline_packages`), regardless of which feature
    wins the dependency, so `low`'s inline definition leaks in and resolution
    wrongly reaches `lowbogusbackend`. A single environment is used so the
    surfaced backend is unambiguous (no cross-environment error ordering).
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
