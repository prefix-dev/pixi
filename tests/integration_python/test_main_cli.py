import os
from pathlib import Path
from .common import verify_cli_command, ExitCode, PIXI_VERSION
import tomllib
import json
import pytest


def test_pixi(pixi: Path) -> None:
    verify_cli_command(
        [pixi], ExitCode.INCORRECT_USAGE, stdout_excludes=f"[version {PIXI_VERSION}]"
    )
    verify_cli_command([pixi, "--version"], ExitCode.SUCCESS, stdout_contains=PIXI_VERSION)


@pytest.mark.slow
def test_project_commands(pixi: Path, tmp_path: Path) -> None:
    manifest_path = tmp_path / "pixi.toml"
    # Create a new project
    verify_cli_command([pixi, "init", tmp_path], ExitCode.SUCCESS)

    # Channel commands
    verify_cli_command(
        [
            pixi,
            "project",
            "--manifest-path",
            manifest_path,
            "channel",
            "add",
            "bioconda",
        ],
        ExitCode.SUCCESS,
    )
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "channel", "list"],
        ExitCode.SUCCESS,
        stdout_contains="bioconda",
    )
    verify_cli_command(
        [
            pixi,
            "project",
            "--manifest-path",
            manifest_path,
            "channel",
            "remove",
            "bioconda",
        ],
        ExitCode.SUCCESS,
    )

    # Description commands
    verify_cli_command(
        [
            pixi,
            "project",
            "--manifest-path",
            manifest_path,
            "description",
            "set",
            "blabla",
        ],
        ExitCode.SUCCESS,
    )
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "description", "get"],
        ExitCode.SUCCESS,
        stdout_contains="blabla",
    )

    # Environment commands
    verify_cli_command(
        [
            pixi,
            "project",
            "--manifest-path",
            manifest_path,
            "environment",
            "add",
            "test",
        ],
        ExitCode.SUCCESS,
    )
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "environment", "list"],
        ExitCode.SUCCESS,
        stdout_contains="test",
    )
    verify_cli_command(
        [
            pixi,
            "project",
            "--manifest-path",
            manifest_path,
            "environment",
            "remove",
            "test",
        ],
        ExitCode.SUCCESS,
    )
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "environment", "list"],
        ExitCode.SUCCESS,
        stdout_excludes="test",
    )

    # Platform commands
    verify_cli_command(
        [
            pixi,
            "project",
            "--manifest-path",
            manifest_path,
            "platform",
            "add",
            "linux-64",
        ],
        ExitCode.SUCCESS,
    )
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "platform", "list"],
        ExitCode.SUCCESS,
        stdout_contains="linux-64",
    )
    verify_cli_command(
        [
            pixi,
            "project",
            "--manifest-path",
            manifest_path,
            "platform",
            "remove",
            "linux-64",
        ],
        ExitCode.SUCCESS,
    )
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "platform", "list"],
        ExitCode.SUCCESS,
        stdout_excludes="linux-64",
    )

    # Version commands
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "version", "set", "1.2.3"],
        ExitCode.SUCCESS,
    )
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "version", "get"],
        ExitCode.SUCCESS,
        stdout_contains="1.2.3",
    )
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "version", "major"],
        ExitCode.SUCCESS,
        stderr_contains="2.2.3",
    )
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "version", "minor"],
        ExitCode.SUCCESS,
        stderr_contains="2.3.3",
    )
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "version", "patch"],
        ExitCode.SUCCESS,
        stderr_contains="2.3.4",
    )


def test_broken_config(pixi: Path, tmp_path: Path) -> None:
    env = {"PIXI_HOME": str(tmp_path)}
    config = tmp_path.joinpath("config.toml")
    toml = """
    [repodata-config."https://prefix.dev/"]
    disable-sharded = false
    """
    config.write_text(toml)

    # Test basic commands
    verify_cli_command([pixi, "info"], env=env)

    toml_invalid = toml + "invalid"
    config.write_text(toml_invalid)
    verify_cli_command([pixi, "info"], env=env, stderr_contains="parse error")

    toml_unknown = "invalid-key = true" + toml
    config.write_text(toml_unknown)
    verify_cli_command([pixi, "info"], env=env, stderr_contains="Ignoring 'invalid-key'")


@pytest.mark.slow
def test_search(pixi: Path) -> None:
    verify_cli_command(
        [pixi, "search", "rattler-build", "-c", "conda-forge"],
        ExitCode.SUCCESS,
        stdout_contains="rattler-build",
    )
    verify_cli_command(
        [pixi, "search", "rattler-build", "-c", "https://fast.prefix.dev/conda-forge"],
        ExitCode.SUCCESS,
        stdout_contains="rattler-build",
    )


@pytest.mark.slow
def test_simple_project_setup(pixi: Path, tmp_path: Path) -> None:
    manifest_path = tmp_path / "pixi.toml"
    conda_forge = "https://fast.prefix.dev/conda-forge"
    # Create a new project
    verify_cli_command([pixi, "init", "-c", conda_forge, tmp_path], ExitCode.SUCCESS)

    # Add package
    verify_cli_command(
        [pixi, "add", "--manifest-path", manifest_path, "_r-mutex"],
        ExitCode.SUCCESS,
        stderr_contains="Added",
    )
    verify_cli_command(
        [
            pixi,
            "add",
            "--manifest-path",
            manifest_path,
            "--feature",
            "test",
            "_r-mutex==1.0.1",
        ],
        ExitCode.SUCCESS,
        stderr_contains=["test", "==1.0.1"],
    )
    verify_cli_command(
        [
            pixi,
            "add",
            "--manifest-path",
            manifest_path,
            "--platform",
            "linux-64",
            f"{conda_forge}::_r-mutex",
        ],
        ExitCode.SUCCESS,
        stderr_contains=["linux-64", "conda-forge"],
    )
    verify_cli_command(
        [
            pixi,
            "add",
            "--manifest-path",
            manifest_path,
            "-f",
            "test",
            "-p",
            "osx-arm64",
            "_r-mutex",
        ],
        ExitCode.SUCCESS,
        stderr_contains=["osx-arm64", "test"],
    )

    # Remove package
    verify_cli_command(
        [pixi, "remove", "--manifest-path", manifest_path, "_r-mutex"],
        ExitCode.SUCCESS,
        stderr_contains="Removed",
    )
    verify_cli_command(
        [
            pixi,
            "remove",
            "--manifest-path",
            manifest_path,
            "--feature",
            "test",
            "_r-mutex",
        ],
        ExitCode.SUCCESS,
        stderr_contains=["test", "Removed"],
    )
    verify_cli_command(
        [
            pixi,
            "remove",
            "--manifest-path",
            manifest_path,
            "--platform",
            "linux-64",
            f"{conda_forge}::_r-mutex",
        ],
        ExitCode.SUCCESS,
        stderr_contains=["linux-64", "conda-forge", "Removed"],
    )
    verify_cli_command(
        [
            pixi,
            "remove",
            "--manifest-path",
            manifest_path,
            "-f",
            "test",
            "-p",
            "osx-arm64",
            "_r-mutex",
        ],
        ExitCode.SUCCESS,
        stderr_contains=["osx-arm64", "test", "Removed"],
    )


@pytest.mark.slow
def test_pixi_init_pyproject(pixi: Path, tmp_path: Path) -> None:
    manifest_path = tmp_path / "pyproject.toml"
    # Create a new project
    verify_cli_command([pixi, "init", tmp_path, "--format", "pyproject"], ExitCode.SUCCESS)
    # Verify that install works
    verify_cli_command([pixi, "install", "--manifest-path", manifest_path], ExitCode.SUCCESS)


def test_upgrade_package_does_not_exist(
    pixi: Path, tmp_path: Path, multiple_versions_channel_1: str
) -> None:
    manifest_path = tmp_path / "pixi.toml"

    # Create a new project
    verify_cli_command([pixi, "init", "--channel", multiple_versions_channel_1, tmp_path])

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
    pixi: Path, tmp_path: Path, multiple_versions_channel_1: str
) -> None:
    manifest_path = tmp_path / "pixi.toml"

    # Create a new project
    verify_cli_command([pixi, "init", "--channel", multiple_versions_channel_1, tmp_path])

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


def test_upgrade_exclude(pixi: Path, tmp_path: Path, multiple_versions_channel_1: str) -> None:
    manifest_path = tmp_path / "pixi.toml"

    # Create a new project
    verify_cli_command([pixi, "init", "--channel", multiple_versions_channel_1, tmp_path])

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


def test_upgrade_json_output(pixi: Path, tmp_path: Path, multiple_versions_channel_1: str) -> None:
    manifest_path = tmp_path / "pixi.toml"

    # Create a new project
    verify_cli_command([pixi, "init", "--channel", multiple_versions_channel_1, tmp_path])

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


def test_upgrade_dryrun(pixi: Path, tmp_path: Path, multiple_versions_channel_1: str) -> None:
    manifest_path = tmp_path / "pixi.toml"
    lock_file_path = tmp_path / "pixi.lock"
    # Create a new project
    verify_cli_command([pixi, "init", "--channel", multiple_versions_channel_1, tmp_path])

    # Add package pinned to version 0.1.0
    verify_cli_command(
        [pixi, "add", "--manifest-path", manifest_path, "package==0.1.0", "package2==0.1.0"]
    )

    manifest_content = manifest_path.read_text()
    lock_file_content = lock_file_path.read_text()
    # Rename .pixi folder, no remove to avoid remove logic.
    os.renames(tmp_path / ".pixi", tmp_path / ".pixi_backup")

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
    assert not os.path.exists(tmp_path / ".pixi")


@pytest.mark.slow
def test_upgrade_pypi_package(pixi: Path, tmp_path: Path) -> None:
    manifest_path = tmp_path / "pixi.toml"

    # Create a new project
    verify_cli_command([pixi, "init", tmp_path])

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
def test_upgrade_pypi_and_conda_package(pixi: Path, tmp_path: Path) -> None:
    manifest_path = tmp_path / "pyproject.toml"

    # Create a new project
    verify_cli_command([pixi, "init", "--format", "pyproject", tmp_path])

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


def test_upgrade_keep_info(pixi: Path, tmp_path: Path, multiple_versions_channel_1: str) -> None:
    manifest_path = tmp_path / "pixi.toml"

    # Create a new project
    verify_cli_command([pixi, "init", "--channel", multiple_versions_channel_1, tmp_path])

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


def test_upgrade_remove_info(pixi: Path, tmp_path: Path, multiple_versions_channel_1: str) -> None:
    manifest_path = tmp_path / "pixi.toml"

    # Create a new project
    verify_cli_command([pixi, "init", "--channel", multiple_versions_channel_1, tmp_path])

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


def test_concurrency_flags(pixi: Path, tmp_path: Path, multiple_versions_channel_1: str) -> None:
    manifest_path = tmp_path / "pixi.toml"

    # Create a new project
    verify_cli_command([pixi, "init", "--channel", multiple_versions_channel_1, tmp_path])

    # Add package pinned to version 0.1.0
    verify_cli_command(
        [
            pixi,
            "add",
            "--concurrent-solves=12",
            "--concurrent-downloads=2",
            "--manifest-path",
            manifest_path,
            "package3",
        ]
    )
