import os
from pathlib import Path
import shutil
import platform

from .common import (
    cwd,
    verify_cli_command,
    ExitCode,
    PIXI_VERSION,
    CURRENT_PLATFORM,
    EMPTY_BOILERPLATE_PROJECT,
)
import tomllib
import json
import pytest


def test_pixi(pixi: Path) -> None:
    verify_cli_command(
        [pixi], ExitCode.INCORRECT_USAGE, stdout_excludes=f"[version {PIXI_VERSION}]"
    )
    verify_cli_command([pixi, "--version"], ExitCode.SUCCESS, stdout_contains=PIXI_VERSION)


@pytest.mark.slow
def test_project_commands(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    # Create a new project
    verify_cli_command([pixi, "init", tmp_pixi_workspace], ExitCode.SUCCESS)

    # Channel commands
    verify_cli_command(
        [
            pixi,
            "project",
            "--manifest-path",
            manifest_path,
            "channel",
            "add",
            "https://prefix.dev/bioconda",
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
            "https://prefix.dev/bioconda",
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
            "osx-arm64",
        ],
        ExitCode.SUCCESS,
    )
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "platform", "list"],
        ExitCode.SUCCESS,
        stdout_contains=["linux-64", "osx-arm64"],
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
        [
            pixi,
            "project",
            "--manifest-path",
            manifest_path,
            "platform",
            "remove",
            "linux-64",
        ],
        ExitCode.FAILURE,
        stderr_contains="linux-64",
    )
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "platform", "list"],
        ExitCode.SUCCESS,
        stdout_contains="osx-arm64",
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
        stderr_contains="2.0.0",
    )
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "version", "minor"],
        ExitCode.SUCCESS,
        stderr_contains="2.1.0",
    )
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "version", "patch"],
        ExitCode.SUCCESS,
        stderr_contains="2.1.1",
    )
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "name", "get"],
        ExitCode.SUCCESS,
        stdout_contains="test_project_commands",
    )
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "name", "set", "new-name"],
        ExitCode.SUCCESS,
        stderr_contains="new-name",
    )


def test_broken_config(pixi: Path, tmp_pixi_workspace: Path) -> None:
    env = {"PIXI_HOME": str(tmp_pixi_workspace)}
    config = tmp_pixi_workspace.joinpath("config.toml")
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
        [pixi, "search", "rattler-build", "-c", "https://prefix.dev/conda-forge"],
        ExitCode.SUCCESS,
        stdout_contains="rattler-build",
    )


@pytest.mark.slow
def test_simple_project_setup(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    conda_forge = "https://prefix.dev/conda-forge"
    # Create a new project
    verify_cli_command([pixi, "init", "-c", conda_forge, tmp_pixi_workspace], ExitCode.SUCCESS)

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


def test_pixi_init_cwd(pixi: Path, tmp_pixi_workspace: Path) -> None:
    # Change directory to workspace
    with cwd(tmp_pixi_workspace):
        # Create a new project
        verify_cli_command([pixi, "init", "."], ExitCode.SUCCESS)

        # Verify that the manifest file is created
        manifest_path = tmp_pixi_workspace / "pixi.toml"
        assert manifest_path.exists()

        # Verify that the manifest file contains expected content
        manifest_content = manifest_path.read_text()
        assert "[workspace]" in manifest_content


def test_pixi_init_non_existing_dir(pixi: Path, tmp_pixi_workspace: Path) -> None:
    # Specify project dir
    project_dir = tmp_pixi_workspace / "project_dir"

    # Create a new project
    verify_cli_command([pixi, "init", project_dir], ExitCode.SUCCESS)

    # Verify that the manifest file is created
    manifest_path = project_dir / "pixi.toml"
    assert manifest_path.exists()

    # Verify that the manifest file contains expected content
    manifest_content = manifest_path.read_text()
    assert "[workspace]" in manifest_content


@pytest.mark.slow
def test_pixi_init_pyproject(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest_path = tmp_pixi_workspace / "pyproject.toml"
    # Create a new project
    verify_cli_command(
        [pixi, "init", tmp_pixi_workspace, "--format", "pyproject"], ExitCode.SUCCESS
    )
    # Verify that install works
    verify_cli_command([pixi, "install", "--manifest-path", manifest_path], ExitCode.SUCCESS)


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

    # Check that `requrires-python` is the same
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


def test_concurrency_flags(
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
            "--concurrent-solves=12",
            "--concurrent-downloads=2",
            "--manifest-path",
            manifest_path,
            "package3",
        ]
    )


def test_dont_add_broken_dep(pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"

    # Create a new project
    verify_cli_command([pixi, "init", "--channel", dummy_channel_1, tmp_pixi_workspace])

    manifest_content = tmp_pixi_workspace.joinpath("pixi.toml").read_text()

    # Add a non existing package should error
    verify_cli_command(
        [pixi, "add", "--manifest-path", manifest_path, "dummy-a=1000000"],
        ExitCode.FAILURE,
    )

    # It should not have modified the manifest on failure
    assert manifest_content == tmp_pixi_workspace.joinpath("pixi.toml").read_text()


def test_list_exits_unsuccessful_on_unknown_pkg(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    # Create a new project
    verify_cli_command([pixi, "init", "--channel", dummy_channel_1, tmp_pixi_workspace])

    # Add a non existing package should error
    verify_cli_command(
        [pixi, "list", "asdfasdf"],
        ExitCode.FAILURE,
    )


def test_pixi_manifest_path(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"

    # Create a new project
    verify_cli_command([pixi, "init", tmp_pixi_workspace], ExitCode.SUCCESS)

    # Modify project without manifest path
    verify_cli_command(
        [
            pixi,
            "project",
            "description",
            "set",
            "blabla",
        ],
        ExitCode.SUCCESS,
        cwd=tmp_pixi_workspace,
    )

    # Verify project by manifest path to 'pixi.toml'
    verify_cli_command(
        [pixi, "project", "--manifest-path", manifest_path, "description", "get"],
        ExitCode.SUCCESS,
        stdout_contains="blabla",
    )

    # Verify project by manifest path to workspace
    verify_cli_command(
        [pixi, "project", "--manifest-path", tmp_pixi_workspace, "description", "get"],
        ExitCode.SUCCESS,
        stdout_contains="blabla",
    )


def test_project_system_requirements(pixi: Path, tmp_pixi_workspace: Path) -> None:
    verify_cli_command([pixi, "init", tmp_pixi_workspace])

    # Add system requirements
    verify_cli_command(
        [
            pixi,
            "project",
            "--manifest-path",
            tmp_pixi_workspace / "pixi.toml",
            "system-requirements",
            "add",
            "cuda",
            "11.8",
        ],
        ExitCode.SUCCESS,
    )
    verify_cli_command(
        [
            pixi,
            "project",
            "--manifest-path",
            tmp_pixi_workspace / "pixi.toml",
            "system-requirements",
            "add",
            "glibc",
            "2.27",
        ],
        ExitCode.SUCCESS,
    )
    verify_cli_command(
        [
            pixi,
            "project",
            "--manifest-path",
            tmp_pixi_workspace / "pixi.toml",
            "system-requirements",
            "add",
            "macos",
            "15.4",
        ],
        ExitCode.SUCCESS,
    )
    verify_cli_command(
        [
            pixi,
            "project",
            "--manifest-path",
            tmp_pixi_workspace / "pixi.toml",
            "system-requirements",
            "add",
            "linux",
            "6.5",
        ],
        ExitCode.SUCCESS,
    )
    verify_cli_command(
        [
            pixi,
            "project",
            "--manifest-path",
            tmp_pixi_workspace / "pixi.toml",
            "system-requirements",
            "add",
            "other-libc",
            "1.2.3",
        ],
        ExitCode.INCORRECT_USAGE,
        stderr_contains="--family",
    )
    verify_cli_command(
        [
            pixi,
            "project",
            "--manifest-path",
            tmp_pixi_workspace / "pixi.toml",
            "system-requirements",
            "add",
            "other-libc",
            "1.2.3",
            "--family",
            "musl",
        ],
        ExitCode.SUCCESS,
    )

    # List system requirements
    verify_cli_command(
        [
            pixi,
            "project",
            "--manifest-path",
            tmp_pixi_workspace / "pixi.toml",
            "system-requirements",
            "list",
        ],
        ExitCode.SUCCESS,
        stdout_contains=["CUDA", "macOS", "Linux", "LibC", "musl"],
    )

    # Add extra environment
    verify_cli_command(
        [
            pixi,
            "project",
            "--manifest-path",
            tmp_pixi_workspace / "pixi.toml",
            "system-requirements",
            "add",
            "--feature",
            "test",
            "linux",
            "10.1",
        ],
        ExitCode.SUCCESS,
    )
    verify_cli_command(
        [
            pixi,
            "project",
            "--manifest-path",
            tmp_pixi_workspace / "pixi.toml",
            "environment",
            "add",
            "test",
            "--feature",
            "test",
        ],
        ExitCode.SUCCESS,
    )

    # List system requirements of environment
    verify_cli_command(
        [
            pixi,
            "project",
            "--manifest-path",
            tmp_pixi_workspace / "pixi.toml",
            "system-requirements",
            "list",
            "--environment",
            "test",
        ],
        ExitCode.SUCCESS,
        stdout_contains=["Linux: 10.1"],
    )


def test_pixi_lock(pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"
    lock_file_path = tmp_pixi_workspace / "pixi.lock"

    # Create a new project
    verify_cli_command([pixi, "init", "--channel", dummy_channel_1, tmp_pixi_workspace])

    # Add a dependency
    verify_cli_command([pixi, "add", "--manifest-path", manifest_path, "dummy-a"])

    # Read the original lock file content
    original_lock_content = lock_file_path.read_text()

    # Remove the lock file
    lock_file_path.unlink()

    # Remove the .pixi folder
    dot_pixi = tmp_pixi_workspace / ".pixi"
    shutil.rmtree(dot_pixi)

    # Run pixi lock to recreate the lock file
    verify_cli_command(
        [pixi, "lock", "--manifest-path", manifest_path], stderr_contains=["+", "dummy-a"]
    )

    # Read the recreated lock file content
    recreated_lock_content = lock_file_path.read_text()

    # Check if the recreated lock file is the same as the original
    assert original_lock_content == recreated_lock_content

    # Ensure the .pixi folder does not exist
    assert not dot_pixi.exists()


@pytest.mark.extra_slow
def test_pixi_auth(pixi: Path) -> None:
    verify_cli_command([pixi, "auth", "login", "--token", "DUMMY_TOKEN", "https://prefix.dev/"])
    verify_cli_command(
        [pixi, "auth", "login", "--token", "DUMMY_TOKEN", "https://repo.prefix.dev/"]
    )
    verify_cli_command(
        [pixi, "auth", "login", "--conda-token", "DUMMY_TOKEN", "https://conda.anaconda.org"]
    )
    verify_cli_command(
        [
            pixi,
            "auth",
            "login",
            "--username",
            "DUMMY_USER",
            "--password",
            "DUMMY_PASS",
            "https://host.org",
        ]
    )
    verify_cli_command(
        [
            pixi,
            "auth",
            "login",
            "--s3-access-key-id",
            "DUMMY_ID",
            "--s3-secret-access-key",
            "DUMMY_KEY",
            "--s3-session-token",
            "DUMMY_TOKEN",
            "s3://amazon-aws.com",
        ]
    )

    verify_cli_command([pixi, "auth", "logout", "https://prefix.dev/"])
    verify_cli_command([pixi, "auth", "logout", "https://repo.prefix.dev/"])
    verify_cli_command([pixi, "auth", "logout", "https://conda.anaconda.org"])
    verify_cli_command([pixi, "auth", "logout", "https://host.org"])
    verify_cli_command([pixi, "auth", "logout", "s3://amazon-aws.com"])


@pytest.mark.extra_slow
def test_adding_git_deps(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest_path = tmp_pixi_workspace / "pixi.toml"

    lock_file = tmp_pixi_workspace / "pixi.lock"

    verify_cli_command([pixi, "init", tmp_pixi_workspace])

    # we want to add three kinds of pypi dependencies
    # branch
    # tag
    # revision
    # we will use the same package for all three
    verify_cli_command([pixi, "add", "--manifest-path", manifest_path, "python"])

    verify_cli_command(
        [
            pixi,
            "add",
            "--manifest-path",
            manifest_path,
            "--pypi",
            "--git",
            "https://github.com/mahmoud/boltons.git",
            "boltons",
            "--branch",
            "master",
        ]
    )

    # we want to make sure that the lock file contains the branch information
    assert "pypi: git+https://github.com/mahmoud/boltons.git?branch=master" in lock_file.read_text()
    # and that the manifest contains the branch information
    manifest = tomllib.loads(manifest_path.read_text())
    assert manifest["pypi-dependencies"]["boltons"]["branch"] == "master"

    # now add a tag
    verify_cli_command(
        [
            pixi,
            "add",
            "--manifest-path",
            manifest_path,
            "--pypi",
            "--git",
            "https://github.com/mahmoud/boltons.git",
            "boltons",
            "--tag",
            "25.0.0",
        ]
    )

    # we want to make sure that the lock file contains the tag information
    assert "pypi: git+https://github.com/mahmoud/boltons.git?tag=25.0.0" in lock_file.read_text()
    # and that the manifest contains the tag information
    manifest = tomllib.loads(manifest_path.read_text())
    assert manifest["pypi-dependencies"]["boltons"]["tag"] == "25.0.0"

    # now add a simple revision (a commit)
    verify_cli_command(
        [
            pixi,
            "add",
            "--manifest-path",
            manifest_path,
            "--pypi",
            "--git",
            "https://github.com/mahmoud/boltons.git",
            "boltons",
            "--rev",
            "d70669a",
        ]
    )

    # we want to make sure that the lock file contains the rev information
    assert "pypi: git+https://github.com/mahmoud/boltons.git?rev=d70669a" in lock_file.read_text()
    # and that the manifest contains the rev information
    manifest = tomllib.loads(manifest_path.read_text())
    assert manifest["pypi-dependencies"]["boltons"]["rev"] == "d70669a"


def test_dont_error_on_missing_platform(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = f"""
        {EMPTY_BOILERPLATE_PROJECT}
        [feature.a.target.zos-z.tasks]
        nonsense = "echo nonsense"

        [target.zos-z.tasks]
        nonsense = "echo nonsense"
        """
    manifest.write_text(toml)
    # This should not error, but should spawn a warning with a helping message.
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
        stderr_contains=["pixi project platform add zos-z"],
    )


def test_shell_hook_autocompletion(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = f"""
        {EMPTY_BOILERPLATE_PROJECT}
        """
    manifest.write_text(toml)

    # PowerShell completions are handled via PowerShell profile, not shell hook
    verify_cli_command(
        [pixi, "shell-hook", "--manifest-path", manifest, "--shell", "powershell"],
        stdout_excludes=[
            "Scripts/_pixi.ps1"
        ],  # PowerShell doesn't source completions in shell hook
    )

    if platform.system() == "Windows":
        # Test cmd.exe completions (Windows-only)
        cmd_comp_dir = ".pixi/envs/default/Scripts"
        tmp_pixi_workspace.joinpath(cmd_comp_dir).mkdir(parents=True, exist_ok=True)
        tmp_pixi_workspace.joinpath(cmd_comp_dir, "pixi.cmd").touch()
        verify_cli_command(
            [pixi, "shell-hook", "--manifest-path", manifest, "--shell", "cmd"],
            stdout_contains=['@SET "Path=', "Scripts", "@PROMPT"],
        )
    else:
        # Test Unix shell completions
        bash_comp_dir = ".pixi/envs/default/share/bash-completion/completions"
        tmp_pixi_workspace.joinpath(bash_comp_dir).mkdir(parents=True, exist_ok=True)
        tmp_pixi_workspace.joinpath(bash_comp_dir, "pixi.sh").touch()
        verify_cli_command(
            [pixi, "shell-hook", "--manifest-path", manifest, "--shell", "bash"],
            stdout_contains=["source", "share/bash-completion/completions"],
        )

        zsh_comp_dir = ".pixi/envs/default/share/zsh/site-functions"
        tmp_pixi_workspace.joinpath(zsh_comp_dir).mkdir(parents=True, exist_ok=True)
        tmp_pixi_workspace.joinpath(zsh_comp_dir, "_pixi").touch()
        verify_cli_command(
            [pixi, "shell-hook", "--manifest-path", manifest, "--shell", "zsh"],
            stdout_contains=["fpath+=", "share/zsh/site-functions", "autoload -Uz compinit"],
        )

        fish_comp_dir = ".pixi/envs/default/share/fish/vendor_completions.d"
        tmp_pixi_workspace.joinpath(fish_comp_dir).mkdir(parents=True, exist_ok=True)
        tmp_pixi_workspace.joinpath(fish_comp_dir, "pixi.fish").touch()
        verify_cli_command(
            [pixi, "shell-hook", "--manifest-path", manifest, "--shell", "fish"],
            stdout_contains=["for file in", "source", "share/fish/vendor_completions.d"],
        )


def test_pixi_info_tasks(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = """
        [workspace]
        name = "test"
        channels = []
        platforms = ["linux-64", "win-64", "osx-64"]

        [tasks]
        foo = "echo foo"

        [target.unix.tasks]
        bar = "echo bar"

        [target.win.tasks]
        bar = "echo bar"
        """
    manifest.write_text(toml)
    verify_cli_command([pixi, "info", "--manifest-path", manifest], stdout_contains="foo, bar")


def test_pixi_task_list_platforms(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = """
        [workspace]
        name = "test"
        channels = []
        platforms = ["linux-64", "win-64", "osx-64", "osx-arm64"]

        [tasks]
        foo = "echo foo"

        [target.unix.tasks]
        bar = "echo bar"

        [target.win.tasks]
        bar = "echo bar"
        """
    manifest.write_text(toml)
    verify_cli_command(
        [pixi, "task", "list", "--manifest-path", manifest], stderr_contains=["foo", "bar"]
    )


def test_pixi_add_alias(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = """
        [workspace]
        name = "test"
        channels = []
        platforms = ["linux-64", "win-64", "osx-64", "osx-arm64"]
        """
    manifest.write_text(toml)

    verify_cli_command(
        [pixi, "task", "alias", "dummy-a", "dummy-b", "dummy-c", "--manifest-path", manifest]
    )
    # Test platform-specific task alias
    verify_cli_command(
        [
            pixi,
            "task",
            "alias",
            "--platform",
            "linux-64",
            "linux-alias",
            "dummy-b",
            "dummy-c",
            "--manifest-path",
            manifest,
        ]
    )

    with open(manifest, "rb") as f:
        manifest_content = tomllib.load(f)

    assert "target" in manifest_content
    assert "linux-64" in manifest_content["target"]
    assert "tasks" in manifest_content["target"]["linux-64"]
    assert "linux-alias" in manifest_content["target"]["linux-64"]["tasks"]
    assert manifest_content["target"]["linux-64"]["tasks"]["linux-alias"] == [
        {"task": "dummy-b"},
        {"task": "dummy-c"},
    ]

    assert "dummy-a" in manifest_content["tasks"]
    assert manifest_content["tasks"]["dummy-a"] == [{"task": "dummy-b"}, {"task": "dummy-c"}]
