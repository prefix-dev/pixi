import json
import os
import tomllib
from pathlib import Path

import pytest

from .common import (
    CURRENT_PLATFORM,
    ExitCode,
    verify_cli_command,
)


def test_upgrade_package_does_not_exist(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"

    # Create a new project
    verify_cli_command([pixi, "init", "--channel", multiple_versions_channel_1, tmp_pixi_workspace])

    # Add package
    verify_cli_command([pixi, "add", "--manifest-path", manifest_path, "package"])

    # Similar package names that don't exist should get suggestions
    verify_cli_command(
        [pixi, "upgrade", "--manifest-path", manifest_path, "package_similar_name"],
        ExitCode.FAILURE,
        stderr_contains=[
            "could not find a package named 'package_similar_name'",
            "did you mean 'package'",
        ],
    )

    verify_cli_command(
        [pixi, "upgrade", "--manifest-path", manifest_path, "different_name"],
        ExitCode.FAILURE,
        stderr_contains="could not find a package named 'different_name'",
        stderr_excludes="did you mean 'package'",
    )


def test_upgrade_conda_package(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"

    # Create a new project
    verify_cli_command([pixi, "init", "--channel", multiple_versions_channel_1, tmp_pixi_workspace])

    # Add package pinned to version 0.1.0
    verify_cli_command(
        [
            pixi,
            "add",
            "--manifest-path",
            manifest_path,
            f"package==0.1.0[channel={multiple_versions_channel_1},build_number=0]",
        ]
    )
    parsed_manifest = tomllib.loads(manifest_path.read_text())
    package = parsed_manifest["dependencies"]["package"]
    assert package["version"] == "==0.1.0"
    assert package["channel"] == multiple_versions_channel_1
    assert package["build-number"] == "==0"

    # Upgrade package, it should now be at 0.2.0, with semver ranges
    # The channel should still be specified
    verify_cli_command(
        [pixi, "upgrade", "--manifest-path", manifest_path, "package"],
        stderr_contains=["package", "0.1.0", "0.2.0"],
    )
    parsed_manifest = tomllib.loads(manifest_path.read_text())
    package = parsed_manifest["dependencies"]["package"]
    assert package["version"] == ">=0.2.0,<0.3"
    assert package["channel"] == multiple_versions_channel_1
    assert "build-number" not in package


def test_upgrade_exclude(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"

    # Create a new project
    verify_cli_command([pixi, "init", "--channel", multiple_versions_channel_1, tmp_pixi_workspace])

    # Add package pinned to version 0.1.0
    verify_cli_command(
        [pixi, "add", "--manifest-path", manifest_path, "package==0.1.0", "package2==0.1.0"]
    )
    parsed_manifest = tomllib.loads(manifest_path.read_text())
    assert parsed_manifest["dependencies"]["package"] == "==0.1.0"
    assert parsed_manifest["dependencies"]["package2"] == "==0.1.0"

    # Upgrade package, it should now be at 0.2.0, with semver ranges
    # package2, should still be at 0.1.0, since we excluded it
    verify_cli_command(
        [pixi, "upgrade", "--manifest-path", manifest_path, "--exclude", "package2"],
        stderr_contains=["package", "0.1.0", "0.2.0"],
        stderr_excludes="package2",
    )
    parsed_manifest = tomllib.loads(manifest_path.read_text())
    assert parsed_manifest["dependencies"]["package"] == ">=0.2.0,<0.3"
    assert parsed_manifest["dependencies"]["package2"] == "==0.1.0"


def test_upgrade_json_output(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"

    # Create a new project
    verify_cli_command([pixi, "init", "--channel", multiple_versions_channel_1, tmp_pixi_workspace])

    # Add package pinned to version 0.1.0
    verify_cli_command(
        [pixi, "add", "--manifest-path", manifest_path, "package==0.1.0", "package2==0.1.0"]
    )
    parsed_manifest = tomllib.loads(manifest_path.read_text())
    assert parsed_manifest["dependencies"]["package"] == "==0.1.0"
    assert parsed_manifest["dependencies"]["package2"] == "==0.1.0"

    # Check if json output is correct and readable
    result = verify_cli_command(
        [pixi, "upgrade", "--manifest-path", manifest_path, "--json"],
        stdout_contains=["package", "package2", "0.1.0", "0.2.0", 'version": ', "before", "after"],
    )

    data = json.loads(result.stdout)
    assert data["environment"]["default"]


def test_upgrade_dryrun(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    lock_file_path = tmp_pixi_workspace / "pixi.lock"
    # Create a new project
    verify_cli_command([pixi, "init", "--channel", multiple_versions_channel_1, tmp_pixi_workspace])

    # Add package pinned to version 0.1.0
    verify_cli_command(
        [pixi, "add", "--manifest-path", manifest_path, "package==0.1.0", "package2==0.1.0"]
    )

    manifest_content = manifest_path.read_text()
    lock_file_content = lock_file_path.read_text()
    # Rename .pixi folder, no remove to avoid remove logic.
    os.renames(tmp_pixi_workspace / ".pixi", tmp_pixi_workspace / ".pixi_backup")

    parsed_manifest = tomllib.loads(manifest_path.read_text())
    assert parsed_manifest["dependencies"]["package"] == "==0.1.0"
    assert parsed_manifest["dependencies"]["package2"] == "==0.1.0"

    verify_cli_command(
        [pixi, "upgrade", "--manifest-path", manifest_path, "--dry-run"],
        stderr_contains=["package", "0.1.0", "0.2.0"],
    )

    # Verify the manifest, lock file and .pixi folder are not modified
    assert manifest_path.read_text() == manifest_content
    assert lock_file_path.read_text() == lock_file_content
    assert not os.path.exists(tmp_pixi_workspace / ".pixi")


@pytest.mark.slow
def test_upgrade_pypi_package(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"

    # Create a new project
    verify_cli_command([pixi, "init", tmp_pixi_workspace])

    # Add python
    verify_cli_command([pixi, "add", "--manifest-path", manifest_path, "python=3.13"])

    # Add httpx pinned to version 0.26.0
    verify_cli_command(
        [
            pixi,
            "add",
            "--manifest-path",
            manifest_path,
            "--pypi",
            "httpx[cli]==0.26.0",
        ]
    )
    parsed_manifest = tomllib.loads(manifest_path.read_text())
    assert parsed_manifest["pypi-dependencies"]["httpx"]["version"] == "==0.26.0"
    assert parsed_manifest["pypi-dependencies"]["httpx"]["extras"] == ["cli"]

    # Upgrade httpx, it should now be upgraded
    # Extras should be preserved
    verify_cli_command(
        [pixi, "upgrade", "--manifest-path", manifest_path, "httpx"],
        stderr_contains=["httpx", "0.26.0"],
    )
    parsed_manifest = tomllib.loads(manifest_path.read_text())
    assert parsed_manifest["pypi-dependencies"]["httpx"]["version"] != "==0.26.0"
    assert parsed_manifest["pypi-dependencies"]["httpx"]["extras"] == ["cli"]


@pytest.mark.slow
def test_upgrade_pypi_and_conda_package(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest_path = tmp_pixi_workspace / "pyproject.toml"

    # Create a new project
    verify_cli_command(
        [
            pixi,
            "init",
            "--format",
            "pyproject",
            tmp_pixi_workspace,
            "--channel",
            "https://prefix.dev/conda-forge",
        ]
    )

    # Add pinned numpy as conda and pypi dependency
    verify_cli_command([pixi, "add", "--manifest-path", manifest_path, "numpy==1.*"])
    verify_cli_command([pixi, "add", "--manifest-path", manifest_path, "--pypi", "numpy==1.*"])

    parsed_manifest = tomllib.loads(manifest_path.read_text())
    numpy_pypi = parsed_manifest["project"]["dependencies"][0]
    assert numpy_pypi == "numpy==1.*"
    numpy_conda = parsed_manifest["tool"]["pixi"]["dependencies"]["numpy"]
    assert numpy_conda == "1.*"

    # Upgrade numpy, both conda and pypi should be upgraded
    verify_cli_command(
        [pixi, "upgrade", "--manifest-path", manifest_path, "numpy"],
        stderr_contains=["numpy", "1."],
    )
    parsed_manifest = tomllib.loads(manifest_path.read_text())
    numpy_pypi = parsed_manifest["project"]["dependencies"][0]
    assert "1.*" not in numpy_pypi
    numpy_conda = parsed_manifest["tool"]["pixi"]["dependencies"]["numpy"]
    assert numpy_conda != "1.*"


@pytest.mark.slow
def test_upgrade_dependency_location_pixi(pixi: Path, tmp_path: Path) -> None:
    # Test based on https://github.com/prefix-dev/pixi/issues/2470
    # Making sure pixi places the upgraded package in the correct location
    manifest_path = tmp_path / "pyproject.toml"
    pyproject = f"""
[project]
name = "test-upgrade"
dependencies = ["numpy==1.*"]
requires-python = "==3.13"

[project.optional-dependencies]
cli = ["rich==12"]

[dependency-groups]
test = ["pytest==6"]

[tool.pixi.project]
channels = ["https://prefix.dev/conda-forge"]
platforms = ["{CURRENT_PLATFORM}"]

[tool.pixi.pypi-dependencies]
polars = "==0.*"

[tool.pixi.environments]
test = ["test"]
    """

    manifest_path.write_text(pyproject)

    # Upgrade numpy, both conda and pypi should be upgraded
    verify_cli_command(
        [pixi, "upgrade", "--manifest-path", manifest_path],
        stderr_contains=["polars"],
    )
    parsed_manifest = tomllib.loads(manifest_path.read_text())

    # Check that `requires-python` is the same
    assert parsed_manifest["project"]["requires-python"] == "==3.13"

    # Check that `tool.pixi.dependencies.python` isn't added
    assert "python" not in parsed_manifest.get("tool", {}).get("pixi", {}).get("dependencies", {})

    # Check that project.dependencies are upgraded
    project_dependencies = parsed_manifest["project"]["dependencies"]
    numpy_pypi = project_dependencies[0]
    assert "numpy" in numpy_pypi
    assert "==1.*" not in numpy_pypi
    assert "polars" not in project_dependencies

    # Check that the pypi-dependencies are upgraded
    pypi_dependencies = parsed_manifest["tool"]["pixi"]["pypi-dependencies"]
    polars_pypi = pypi_dependencies["polars"]
    assert polars_pypi != "==0.*"
    assert "numpy" not in pypi_dependencies


def test_upgrade_keep_info(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"

    # Create a new project
    verify_cli_command([pixi, "init", "--channel", multiple_versions_channel_1, tmp_pixi_workspace])

    # Add package pinned to version 0.1.0
    verify_cli_command(
        [
            pixi,
            "add",
            "--manifest-path",
            manifest_path,
            f"{multiple_versions_channel_1}::package3==0.1.0=ab*",
        ]
    )
    parsed_manifest = tomllib.loads(manifest_path.read_text())
    assert "==0.1.0" in parsed_manifest["dependencies"]["package3"]["version"]
    assert "ab*" in parsed_manifest["dependencies"]["package3"]["build"]
    assert multiple_versions_channel_1 in parsed_manifest["dependencies"]["package3"]["channel"]

    # Upgrade all, it should now be at 0.2.0, with the build intact
    verify_cli_command(
        [pixi, "upgrade", "--manifest-path", manifest_path],
        stderr_contains=["package3", "0.1.0", "0.2.0"],
    )
    parsed_manifest = tomllib.loads(manifest_path.read_text())
    # Update version
    assert parsed_manifest["dependencies"]["package3"]["version"] == ">=0.2.0,<0.3"
    # Keep build
    assert "ab*" in parsed_manifest["dependencies"]["package3"]["build"]
    # Keep channel
    assert multiple_versions_channel_1 in parsed_manifest["dependencies"]["package3"]["channel"]

    # Upgrade package3, it should now be at 0.2.0, with the build intact because it has a wildcard
    verify_cli_command(
        [pixi, "upgrade", "--manifest-path", manifest_path, "package3"],
    )
    parsed_manifest = tomllib.loads(manifest_path.read_text())
    # Update version
    assert parsed_manifest["dependencies"]["package3"]["version"] == ">=0.2.0,<0.3"
    # Keep build
    assert "ab*" in parsed_manifest["dependencies"]["package3"]["build"]
    # Keep channel
    assert multiple_versions_channel_1 in parsed_manifest["dependencies"]["package3"]["channel"]


def test_upgrade_remove_info(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"

    # Create a new project
    verify_cli_command([pixi, "init", "--channel", multiple_versions_channel_1, tmp_pixi_workspace])

    # Add package pinned to version 0.1.0
    verify_cli_command(
        [
            pixi,
            "add",
            "--manifest-path",
            manifest_path,
            f"{multiple_versions_channel_1}::package3==0.1.0=abc",
        ]
    )
    parsed_manifest = tomllib.loads(manifest_path.read_text())
    assert "==0.1.0" in parsed_manifest["dependencies"]["package3"]["version"]
    assert "abc" in parsed_manifest["dependencies"]["package3"]["build"]
    assert multiple_versions_channel_1 in parsed_manifest["dependencies"]["package3"]["channel"]

    # Upgrade package3, it should now be at 0.2.0, without the build but with the channel
    verify_cli_command(
        [pixi, "upgrade", "--manifest-path", manifest_path, "package3"],
    )
    parsed_manifest = tomllib.loads(manifest_path.read_text())
    # Update version
    assert parsed_manifest["dependencies"]["package3"]["version"] == ">=0.2.0,<0.3"
    # Keep channel
    assert multiple_versions_channel_1 in parsed_manifest["dependencies"]["package3"]["channel"]
    # Remove build
    assert "build" not in parsed_manifest["dependencies"]["package3"]


def test_upgrade_features(
    pixi: Path, tmp_pixi_workspace: Path, multiple_versions_channel_1: str
) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"

    # Create a new project
    verify_cli_command([pixi, "init", "--channel", multiple_versions_channel_1, tmp_pixi_workspace])

    # Add package3 pinned to version 0.1.0 to feature "foo"
    verify_cli_command(
        [
            pixi,
            "add",
            "--manifest-path",
            manifest_path,
            "--feature",
            "foo",
            f"package3==0.1.0[channel={multiple_versions_channel_1}]",
        ]
    )
    parsed_manifest = tomllib.loads(manifest_path.read_text())
    package3 = parsed_manifest["feature"]["foo"]["dependencies"]["package3"]
    assert package3["version"] == "==0.1.0"
    assert package3["channel"] == multiple_versions_channel_1

    # Add package2 pinned to version 0.1.0 to feature "bar"
    verify_cli_command(
        [
            pixi,
            "add",
            "--manifest-path",
            manifest_path,
            "--feature",
            "bar",
            f"package2==0.1.0[channel={multiple_versions_channel_1}]",
        ]
    )
    parsed_manifest = tomllib.loads(manifest_path.read_text())
    package2 = parsed_manifest["feature"]["bar"]["dependencies"]["package2"]
    assert package2["version"] == "==0.1.0"
    assert package2["channel"] == multiple_versions_channel_1

    # Add package pinned to version 0.1.0 to default feature
    verify_cli_command(
        [
            pixi,
            "add",
            "--manifest-path",
            manifest_path,
            f"package==0.1.0[channel={multiple_versions_channel_1}]",
        ]
    )
    parsed_manifest = tomllib.loads(manifest_path.read_text())
    package = parsed_manifest["dependencies"]["package"]
    assert package["version"] == "==0.1.0"
    assert package["channel"] == multiple_versions_channel_1

    # make features used
    verify_cli_command(
        [
            pixi,
            "workspace",
            "environment",
            "add",
            "--manifest-path",
            manifest_path,
            "--force",
            "default",
            "--feature=foo",
            "--feature=bar",
        ]
    )

    # lock before upgrades
    verify_cli_command(
        [
            pixi,
            "lock",
            "--manifest-path",
            manifest_path,
        ]
    )

    # Upgrading with `--feature=default` should only upgrade the package in the default feature
    verify_cli_command(
        [pixi, "upgrade", "--manifest-path", manifest_path, "--feature=default"],
        stderr_excludes=["package3", "package2"],
        stderr_contains=["package", "0.1.0", "0.2.0"],
    )
    parsed_manifest = tomllib.loads(manifest_path.read_text())
    package3 = parsed_manifest["feature"]["foo"]["dependencies"]["package3"]
    package2 = parsed_manifest["feature"]["bar"]["dependencies"]["package2"]
    package = parsed_manifest["dependencies"]["package"]
    assert package3["version"] == package2["version"] == "==0.1.0"
    assert package["version"] == ">=0.2.0,<0.3"

    # Upgrading with `--feature=foo` should not upgrade the package in feature "bar"
    verify_cli_command(
        [pixi, "upgrade", "--manifest-path", manifest_path, "--feature=foo"],
        stderr_excludes=["package2"],
        stderr_contains=["package3", "0.1.0", "0.2.0"],
    )
    parsed_manifest = tomllib.loads(manifest_path.read_text())
    package3 = parsed_manifest["feature"]["foo"]["dependencies"]["package3"]
    package2 = parsed_manifest["feature"]["bar"]["dependencies"]["package2"]
    assert package2["version"] == "==0.1.0"
    assert package3["version"] == ">=0.2.0,<0.3"

    # Upgrading with no specified feature should upgrade all features (hence "package2" in feature "bar")
    verify_cli_command(
        [pixi, "upgrade", "--manifest-path", manifest_path],
        stderr_contains=["package2", "0.1.0", "0.2.0"],
    )
    parsed_manifest = tomllib.loads(manifest_path.read_text())
    package2 = parsed_manifest["feature"]["bar"]["dependencies"]["package2"]
    assert package2["version"] == ">=0.2.0,<0.3"
