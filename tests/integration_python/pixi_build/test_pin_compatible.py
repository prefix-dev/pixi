"""End-to-end tests for `pin-compatible` specs in package manifests."""

from pathlib import Path
from typing import Any

import pytest
import tomli_w

from .common import (
    CURRENT_PLATFORM,
    verify_cli_command,
)

BACKEND_CHANNELS = [
    "https://prefix.dev/pixi-build-backends",
    "https://prefix.dev/conda-forge",
]


def _python_package_sources(pkg_dir: Path, name: str) -> None:
    pyproject = {
        "project": {
            "name": name,
            "version": "1.0.0",
            "requires-python": ">= 3.11",
        },
        "build-system": {
            "build-backend": "hatchling.build",
            "requires": ["hatchling"],
        },
    }
    pkg_dir.joinpath("pyproject.toml").write_text(tomli_w.dumps(pyproject))
    src = pkg_dir.joinpath("src", name)
    src.mkdir(parents=True, exist_ok=True)
    src.joinpath("__init__.py").write_text(f'def main() -> None:\n    print("hello from {name}")\n')


def _package_manifest(name: str, extra: dict[str, Any]) -> dict[str, Any]:
    return {
        "package": {
            "name": name,
            "version": "1.0.0",
            "build": {
                "backend": {
                    "name": "pixi-build-python",
                    "channels": BACKEND_CHANNELS,
                    "version": "*",
                },
                "config": {"noarch": True},
            },
            "host-dependencies": {"hatchling": "*", "python": ">=3.11"},
            **extra,
        }
    }


def _write_sibling_pin_workspace(root: Path) -> None:
    """A workspace where `wsb` host-depends on its sibling path package
    `wsa` and pins it in its run dependencies."""
    workspace_manifest: dict[str, Any] = {
        "workspace": {
            "channels": ["https://prefix.dev/conda-forge"],
            "preview": ["pixi-build"],
            "platforms": [CURRENT_PLATFORM],
        },
        "dependencies": {"wsb": {"path": "wsb"}},
    }
    root.joinpath("pixi.toml").write_text(tomli_w.dumps(workspace_manifest))

    wsa_dir = root.joinpath("wsa")
    wsa_dir.mkdir()
    wsa_dir.joinpath("pixi.toml").write_text(tomli_w.dumps(_package_manifest("wsa", {})))
    _python_package_sources(wsa_dir, "wsa")

    wsb_dir = root.joinpath("wsb")
    wsb_dir.mkdir()
    wsb_manifest = _package_manifest(
        "wsb",
        {
            "host-dependencies": {
                "hatchling": "*",
                "python": ">=3.11",
                "wsa": {"path": "../wsa"},
            },
            "run-dependencies": {
                "python": ">=3.11",
                "wsa": {"pin-compatible": True},
            },
        },
    )
    wsb_dir.joinpath("pixi.toml").write_text(tomli_w.dumps(wsb_manifest))
    _python_package_sources(wsb_dir, "wsb")


@pytest.mark.slow
def test_pin_compatible_on_sibling_source_package_stays_satisfied(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """A pin against a sibling path package must not invalidate a fresh lock.

    Path-based sources are locked as partial records that carry no version.
    Re-deriving `wsb`'s run dependencies during the satisfiability check has
    to resolve `wsa` from the backend to apply the pin. Without that,
    `pixi lock --check` fails on the lock `pixi lock` just wrote, every time.
    """
    _write_sibling_pin_workspace(tmp_pixi_workspace)

    verify_cli_command([pixi, "lock", "--manifest-path", str(tmp_pixi_workspace)])

    lockfile = tmp_pixi_workspace.joinpath("pixi.lock").read_text()
    assert "wsa >=1.0.0,<2.0a0" in lockfile, (
        f"the pin must resolve against the host env's wsa, got:\n{lockfile}"
    )

    # A second check against the fresh lockfile must find no changes.
    verify_cli_command([pixi, "lock", "--check", "--manifest-path", str(tmp_pixi_workspace)])
