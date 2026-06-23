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
