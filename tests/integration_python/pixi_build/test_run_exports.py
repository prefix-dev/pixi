"""End-to-end tests for `[package.run-exports]` in built conda packages.

These tests build real packages with the workspace backends and inspect the
produced `.conda` archives: the exporter's `info/run_exports.json` must record
every declared bucket, and a consumer's `info/index.json` must receive the
exported run dependencies through the conda run-exports mechanism.
"""

import json
import subprocess
import tempfile
from pathlib import Path
from typing import Any

import pytest
import tomli_w
from rattler.package_streaming import extract

from .common import (
    CURRENT_PLATFORM,
    verify_cli_command,
)


BACKEND_CHANNELS = [
    "https://prefix.dev/pixi-build-backends",
    "https://prefix.dev/conda-forge",
]


def _python_package_sources(pkg_dir: Path, name: str) -> None:
    module = name.replace("-", "_")
    pyproject = {
        "project": {
            "name": name,
            "version": "1.0.0",
            "requires-python": ">= 3.11",
            "dependencies": [],
            "scripts": {name: f"{module}:main"},
        },
        "build-system": {
            "build-backend": "hatchling.build",
            "requires": ["hatchling"],
        },
    }
    pkg_dir.joinpath("pyproject.toml").write_text(tomli_w.dumps(pyproject))
    src = pkg_dir.joinpath("src", module)
    src.mkdir(parents=True, exist_ok=True)
    src.joinpath("__init__.py").write_text(f'def main() -> None:\n    print("hello from {name}")\n')


def _package_manifest(name: str, extra: dict[str, Any]) -> dict[str, Any]:
    manifest: dict[str, Any] = {
        "package": {
            "name": name,
            "version": "1.0.0",
            "build": {
                "backend": {
                    "name": "pixi-build-python",
                    "channels": BACKEND_CHANNELS,
                    "version": "*",
                },
            },
            "host-dependencies": {"hatchling": "*"},
            **extra,
        }
    }
    return manifest


def _write_run_exports_workspace(root: Path) -> None:
    """A workspace with an `exporter` package declaring every run-export
    bucket and a `consumer` package that host-depends on it."""
    workspace_manifest: dict[str, Any] = {
        "workspace": {
            "channels": ["https://prefix.dev/conda-forge"],
            "preview": ["pixi-build"],
            "platforms": [CURRENT_PLATFORM],
        },
        "dependencies": {"consumer": {"path": "consumer"}},
    }
    root.joinpath("pixi.toml").write_text(tomli_w.dumps(workspace_manifest))

    exporter_dir = root.joinpath("exporter")
    exporter_dir.mkdir()
    exporter_manifest = _package_manifest(
        "exporter",
        {
            "run-exports": {
                # The consumer is a noarch python package, so only this
                # bucket must reach it. The exporter exports itself as a
                # source spec.
                "noarch": {"exporter": {"path": "."}},
                "weak": {"libzlib": ">=1.2"},
                "strong": {"xz": ">=5"},
                "strong-constraints": {"bzip2": ">=1"},
                "weak-constraints": {"tk": ">=8.6"},
            },
        },
    )
    exporter_dir.joinpath("pixi.toml").write_text(tomli_w.dumps(exporter_manifest))
    _python_package_sources(exporter_dir, "exporter")

    consumer_dir = root.joinpath("consumer")
    consumer_dir.mkdir()
    consumer_manifest = _package_manifest(
        "consumer",
        {
            "host-dependencies": {
                "hatchling": "*",
                "exporter": {"path": "../exporter"},
            },
            # Edge case: an empty run-exports table must parse and build.
            "run-exports": {},
        },
    )
    consumer_dir.joinpath("pixi.toml").write_text(tomli_w.dumps(consumer_manifest))
    _python_package_sources(consumer_dir, "consumer")


def _build_package(pixi: Path, package_dir: Path, target_dir: Path) -> Path:
    verify_cli_command(
        [pixi, "build", "--output-dir", str(target_dir)],
        cwd=package_dir,
    )
    packages = list(target_dir.glob("*.conda"))
    assert len(packages) == 1, f"expected exactly one built package, got {packages}"
    return packages[0]


def _read_info_json(archive: Path, name: str) -> dict[str, Any] | None:
    with tempfile.TemporaryDirectory() as temp_dir:
        extracted = Path(temp_dir)
        extract(archive, extracted)
        info_file = extracted.joinpath("info", name)
        if not info_file.is_file():
            return None
        return json.loads(info_file.read_text())


def _normalized(specs: list[str]) -> set[str]:
    return {spec.replace(" ", "") for spec in specs}


@pytest.mark.slow
def test_built_package_records_declared_run_exports(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """Every declared `[package.run-exports.*]` bucket lands in the built
    package's `info/run_exports.json`, and none of them leaks into the
    exporter's own run dependencies."""
    _write_run_exports_workspace(tmp_pixi_workspace)

    dist = tmp_pixi_workspace.joinpath("dist-exporter")
    archive = _build_package(pixi, tmp_pixi_workspace.joinpath("exporter"), dist)

    run_exports = _read_info_json(archive, "run_exports.json")
    assert run_exports is not None, "the built exporter package must carry run_exports.json"

    assert "libzlib>=1.2" in _normalized(run_exports.get("weak", [])), run_exports
    assert "xz>=5" in _normalized(run_exports.get("strong", [])), run_exports
    assert "bzip2>=1" in _normalized(run_exports.get("strong_constrains", [])), run_exports
    assert "tk>=8.6" in _normalized(run_exports.get("weak_constrains", [])), run_exports
    noarch = run_exports.get("noarch", [])
    assert any(spec.split(" ")[0] == "exporter" for spec in noarch), run_exports

    # The exporter's own run dependencies must not contain its run-exports.
    index = _read_info_json(archive, "index.json")
    assert index is not None
    exported_names = {"libzlib", "xz", "bzip2", "tk"}
    own_dependency_names = {spec.split(" ")[0] for spec in index.get("depends", [])}
    assert not exported_names & own_dependency_names, index.get("depends")


@pytest.mark.slow
def test_consumer_package_receives_noarch_run_export(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """Building a noarch consumer that host-depends on the exporter injects
    the `noarch` bucket - and only that bucket - into the consumer's run
    dependencies recorded in `info/index.json`."""
    _write_run_exports_workspace(tmp_pixi_workspace)

    dist = tmp_pixi_workspace.joinpath("dist-consumer")
    archive = _build_package(pixi, tmp_pixi_workspace.joinpath("consumer"), dist)

    index = _read_info_json(archive, "index.json")
    assert index is not None
    depends = index.get("depends", [])
    dependency_names = {spec.split(" ")[0] for spec in depends}

    assert "exporter" in dependency_names, (
        f"the noarch run-export of the host dependency must become a run dependency, "
        f"got {depends}"
    )
    # A noarch consumer receives only the noarch bucket.
    leaked = {"libzlib", "xz"} & dependency_names
    assert not leaked, f"non-noarch run-export buckets leaked into a noarch consumer: {depends}"


@pytest.mark.slow
def test_lockfile_with_run_export_injected_dependencies_stays_satisfied(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """Run-export-injected dependencies must not invalidate a fresh lock.

    The consumer's locked record carries `exporter` as a run dependency only
    through the run-export of a mutable source host dependency, whose
    run-exports live on the partial record in the lock file. A
    satisfiability check that cannot re-derive that dependency would relock
    (or fail `pixi lock --check`) on every invocation.
    """
    _write_run_exports_workspace(tmp_pixi_workspace)

    verify_cli_command(
        [pixi, "lock", "--manifest-path", str(tmp_pixi_workspace)],
    )
    lockfile = tmp_pixi_workspace.joinpath("pixi.lock").read_text()
    assert "exporter" in lockfile, "the run-export-injected dependency must be locked"

    # A second check against the fresh lockfile must find no changes.
    verify_cli_command(
        [pixi, "lock", "--check", "--manifest-path", str(tmp_pixi_workspace)],
    )


@pytest.mark.slow
def test_rattler_build_backend_does_not_silently_drop_run_exports(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    """`[package.run-exports]` with the rattler-build backend must not vanish.

    The rattler-build backend takes requirements from `recipe.yaml` and
    rejects binary specs in the manifest dependency tables with a clear
    error. Declared run-exports get the same treatment: either they are
    honored in the built package or the build fails loudly; silently
    dropping them is the exact failure mode the pixi-build-api-version 6
    bump exists to prevent.
    """
    workspace_manifest: dict[str, Any] = {
        "workspace": {
            "channels": ["https://prefix.dev/conda-forge"],
            "preview": ["pixi-build"],
            "platforms": [CURRENT_PLATFORM],
        },
        "dependencies": {"pkg": {"path": "pkg"}},
    }
    tmp_pixi_workspace.joinpath("pixi.toml").write_text(tomli_w.dumps(workspace_manifest))
    pkg_dir = tmp_pixi_workspace.joinpath("pkg")
    pkg_dir.mkdir()
    package_manifest: dict[str, Any] = {
        "package": {
            "name": "pkg",
            "version": "1.0.0",
            "build": {
                "backend": {
                    "name": "pixi-build-rattler-build",
                    "channels": BACKEND_CHANNELS,
                    "version": "*",
                },
            },
            "run-exports": {"weak": {"libzlib": ">=1.2"}},
        }
    }
    pkg_dir.joinpath("pixi.toml").write_text(tomli_w.dumps(package_manifest))
    pkg_dir.joinpath("recipe.yaml").write_text("package:\n  name: pkg\n  version: 1.0.0\n")

    dist = tmp_pixi_workspace.joinpath("dist")
    result = subprocess.run(
        [str(pixi), "build", "--output-dir", str(dist)],
        cwd=pkg_dir,
        capture_output=True,
        text=True,
    )
    if result.returncode == 0:
        packages = list(dist.glob("*.conda"))
        assert len(packages) == 1
        run_exports = _read_info_json(packages[0], "run_exports.json")
        assert run_exports is not None and "libzlib>=1.2" in _normalized(
            run_exports.get("weak", [])
        ), (
            "the declared weak run-export was silently dropped from the built package; "
            f"run_exports.json: {run_exports}"
        )
    else:
        combined_output = result.stdout + result.stderr
        assert "run-exports" in combined_output, (
            "the build failed for a reason other than the declared run-exports; "
            f"output: {combined_output}"
        )
