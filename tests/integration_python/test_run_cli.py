import json
import tomli_w
from pathlib import Path

from .common import (
    EMPTY_BOILERPLATE_PROJECT,
    verify_cli_command,
    ExitCode,
    default_env_path,
)

import tempfile
import os
import tomli


def test_run_in_shell_environment(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = f"""
    {EMPTY_BOILERPLATE_PROJECT}
    [tasks]
    task = "echo default"
    task1 = "echo default1"
    [feature.a.tasks]
    task = {{ cmd = "echo a", depends-on = "task1" }}
    task1 = "echo a1"

    [environments]
    a = ["a"]
    """
    manifest.write_text(toml)

    # Run the default task
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "--environment", "default", "task"],
        stdout_contains="default",
        stderr_excludes="default1",
    )

    # Run the a task
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "--environment", "a", "task"],
        stdout_contains=["a", "a1"],
    )

    # Error on non-specified environment as ambiguous
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "task"],
        ExitCode.FAILURE,
        stderr_contains=["ambiguous", "default", "a"],
    )

    # Simulate activated shell in environment 'a'
    env = {"PIXI_IN_SHELL": "true", "PIXI_ENVIRONMENT_NAME": "a"}
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "task"],
        stdout_contains=["a", "a1"],
        env=env,
    )


def test_run_in_shell_project(pixi: Path) -> None:
    # We don't want a `pixi.toml` in our parent directory
    # so let's use tempfile here
    with tempfile.TemporaryDirectory() as tmp_str:
        tmp_pixi_workspace = Path(tmp_str)
        manifest_1_dir = tmp_pixi_workspace.joinpath("manifest_1")
        manifest_1_dir.mkdir()
        manifest_1 = manifest_1_dir.joinpath("pixi.toml")
        toml = f"""
        {EMPTY_BOILERPLATE_PROJECT}
        [tasks]
        task = "echo manifest_1"
        """
        manifest_1.write_text(toml)

        manifest_2_dir = tmp_pixi_workspace.joinpath("manifest_2")
        manifest_2_dir.mkdir()
        manifest_2 = manifest_2_dir.joinpath("pixi.toml")
        toml = f"""
        {EMPTY_BOILERPLATE_PROJECT}
        [tasks]
        task = "echo manifest_2"
        """
        manifest_2.write_text(toml)

        base_env = dict(os.environ)
        base_env.pop("PIXI_IN_SHELL", None)
        base_env.pop("PIXI_PROJECT_MANIFEST", None)
        extended_env = base_env | {
            "PIXI_IN_SHELL": "true",
            "PIXI_PROJECT_MANIFEST": str(manifest_2),
        }

        # Run task with PIXI_PROJECT_MANIFEST set to manifest_2
        verify_cli_command(
            [pixi, "run", "task"],
            stdout_contains="manifest_2",
            env=extended_env,
            cwd=tmp_pixi_workspace,
            reset_env=True,
        )

        # Run with working directory at manifest_1_dir
        verify_cli_command(
            [pixi, "run", "task"],
            stdout_contains="manifest_1",
            env=base_env,
            cwd=manifest_1_dir,
            reset_env=True,
        )

        # Run task with PIXI_PROJECT_MANIFEST set to manifest_2 and working directory at manifest_1_dir
        # working directory should win
        # pixi should warn that it uses the local manifest rather than PIXI_PROJECT_MANIFEST
        verify_cli_command(
            [pixi, "run", "task"],
            stdout_contains="manifest_1",
            stderr_contains="manifest_2",
            env=extended_env,
            cwd=manifest_1_dir,
            reset_env=True,
        )


def test_using_prefix_validation(
    pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str
) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = f"""
    [project]
    name = "test"
    channels = ["{dummy_channel_1}"]
    platforms = ["linux-64", "osx-64", "osx-arm64", "win-64"]

    [dependencies]
    dummy-a = "*"
    """
    manifest.write_text(toml)

    # Run the install
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
    )

    # Validate creation of the pixi file with the hash
    pixi_file = default_env_path(tmp_pixi_workspace).joinpath("conda-meta").joinpath("pixi")
    assert pixi_file.exists()
    assert "environment_lock_file_hash" in pixi_file.read_text()

    # Break environment on purpose
    dummy_a_meta_files = (
        default_env_path(tmp_pixi_workspace).joinpath("conda-meta").glob("dummy-a*.json")
    )

    for file in dummy_a_meta_files:
        path = Path(file)
        if path.exists():
            path.unlink()  # Removes the file

    # Run simple script, which shouldn't reinstall
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "echo", "hello"],
        stdout_contains="hello",
    )

    # Validate that the dummy-a files still don't exist
    for file in dummy_a_meta_files:
        assert not Path(file).exists()

    # Run an actual re-install
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
    )

    # Validate the files are back
    for file in dummy_a_meta_files:
        # All dummy-a files should be back as `install` will ignore the hash
        assert Path(file).exists()


def test_prefix_revalidation(pixi: Path, tmp_pixi_workspace: Path, dummy_channel_1: str) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = f"""
    [project]
    name = "test"
    channels = ["{dummy_channel_1}"]
    platforms = ["linux-64", "osx-64", "osx-arm64", "win-64"]

    [dependencies]
    dummy-a = "*"
    """
    manifest.write_text(toml)

    # Run the installation
    verify_cli_command(
        [pixi, "install", "--manifest-path", manifest],
    )

    # Validate creation of the pixi file with the hash
    pixi_file = default_env_path(tmp_pixi_workspace).joinpath("conda-meta").joinpath("pixi")
    assert pixi_file.exists()
    assert "environment_lock_file_hash" in pixi_file.read_text()

    # Break environment on purpose
    dummy_a_meta_files = (
        default_env_path(tmp_pixi_workspace).joinpath("conda-meta").glob("dummy-a*.json")
    )

    for file in dummy_a_meta_files:
        path = Path(file)
        if path.exists():
            path.unlink()  # Removes the file

    # Run with revalidation to force reinstallation
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "--revalidate", "echo", "hello"],
        stdout_contains="hello",
    )

    # Validate that the dummy-a files are reinstalled
    for file in dummy_a_meta_files:
        assert Path(file).exists()


def test_run_with_activation(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = f"""
    {EMPTY_BOILERPLATE_PROJECT}
    [activation.env]
    TEST_ENV_VAR_FOR_ACTIVATION_TEST = "test123"
    [tasks]
    task = "echo $TEST_ENV_VAR_FOR_ACTIVATION_TEST"
    """
    manifest.write_text(toml)

    # Run the default task
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "task"],
        stdout_contains="test123",
    )

    # Validate that without experimental it does not use the cache
    assert not tmp_pixi_workspace.joinpath(".pixi/activation-env-v0").exists()

    # Enable the experimental cache config
    verify_cli_command(
        [
            pixi,
            "config",
            "set",
            "--manifest-path",
            manifest,
            "--local",
            "experimental.use-environment-activation-cache",
            "true",
        ],
    )

    # Run the default task and create cache
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "task"],
        stdout_contains="test123",
    )

    # Modify the environment variable in cache
    cache_path = tmp_pixi_workspace.joinpath(
        ".pixi", "activation-env-v0", "activation_default.json"
    )
    data = json.loads(cache_path.read_text())
    data["environment_variables"]["TEST_ENV_VAR_FOR_ACTIVATION_TEST"] = "test456"
    cache_path.write_text(json.dumps(data, indent=4))

    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "task"],
        # Contain overwritten value
        stdout_contains="test456",
        stdout_excludes="test123",
    )

    # Ignore activation cache
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "--force-activate", "task", "-vvv"],
        stdout_contains="test123",
    )


def test_detached_environments_run(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    tmp_project = tmp_path.joinpath("pixi-project")
    tmp_project.mkdir()
    detached_envs_tmp = tmp_path.joinpath("pixi-detached-envs")
    manifest = tmp_project.joinpath("pixi.toml")

    # Create a dummy project
    verify_cli_command([pixi, "init", tmp_project, "--channel", dummy_channel_1])
    verify_cli_command([pixi, "add", "dummy-a", "--no-install", "--manifest-path", manifest])

    # Set detached environments
    verify_cli_command(
        [
            pixi,
            "config",
            "set",
            "--manifest-path",
            manifest,
            "--local",
            "detached-environments",
            str(detached_envs_tmp),
        ],
    )

    # Run the installation
    verify_cli_command([pixi, "install", "--manifest-path", manifest])

    # Validate the detached environment
    assert detached_envs_tmp.exists()

    detached_envs_folder = None
    for folder in detached_envs_tmp.iterdir():
        if folder.is_dir():
            detached_envs_folder = folder
            break
    assert detached_envs_folder is not None, "Couldn't find detached environment folder"

    # Validate the conda-meta folder exists
    assert Path(detached_envs_folder).joinpath("envs", "default", "conda-meta").exists()

    # Verify that the detached environment is used
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "echo $CONDA_PREFIX"],
        stdout_contains=f"{detached_envs_tmp}",
    )


def test_run_help(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    manifest.write_text(EMPTY_BOILERPLATE_PROJECT)

    help_long = verify_cli_command(
        [pixi, "run", "--help"],
        stdout_contains="pixi run",
    ).stdout

    help_short = verify_cli_command(
        [pixi, "run", "-h"],
        stdout_contains="pixi run",
    ).stdout

    assert len(help_long.splitlines()) > len(help_short.splitlines())

    help_run = verify_cli_command(
        [pixi, "help", "run"],
        stdout_contains="pixi run",
    ).stdout

    assert help_run == help_long

    verify_cli_command(
        [pixi, "run", "python", "--help"],
        stdout_contains="python",
    )


def test_run_dry_run(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = f"""
    {EMPTY_BOILERPLATE_PROJECT}
    [activation.env]
    DRY_RUN_TEST_VAR = "WET"
    [tasks]
    dry-run-task = "echo $DRY_RUN_TEST_VAR"
    """
    manifest.write_text(toml)

    # Run the task with --dry-run flag
    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest, "--dry-run", "dry-run-task"],
        stderr_contains="$DRY_RUN_TEST_VAR",
        stdout_excludes="WET",
        stderr_excludes="WET",
    )


def test_run_args(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest = tmp_pixi_workspace.joinpath("pixi.toml")
    toml = f"""
    {EMPTY_BOILERPLATE_PROJECT}
    [tasks]
    """
    manifest.write_text(toml)


def test_invalid_task_args(pixi: Path, tmp_pixi_workspace: Path) -> None:
    manifest_path = tmp_pixi_workspace.joinpath("pixi.toml")

    manifest_content = tomli.loads(EMPTY_BOILERPLATE_PROJECT)

    manifest_content["tasks"] = {
        "task_invalid_defaults": {
            "cmd": "echo Invalid defaults: {{ arg1 }} {{ arg2 }} {{ arg3 }}",
            "args": [
                {"arg": "arg1", "default": "default1"},
                "arg2",
                {"arg": "arg3", "default": "default3"},
            ],
        }
    }

    manifest_path.write_text(tomli_w.dumps(manifest_content))

    verify_cli_command(
        [
            pixi,
            "run",
            "--manifest-path",
            manifest_path,
            "task_invalid_defaults",
            "arg1",
            "arg2",
            "arg3",
        ],
        ExitCode.FAILURE,
        stderr_contains="expected default value required after previous arguments with defaults",
    )


def test_task_args_with_defaults(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Test tasks with all default arguments."""
    manifest_path = tmp_pixi_workspace.joinpath("pixi.toml")

    manifest_content = tomli.loads(EMPTY_BOILERPLATE_PROJECT)

    manifest_content["tasks"] = {
        "task_with_defaults": {
            "cmd": "echo Running task with {{ arg1 }} and {{ arg2 }} and {{ arg3 }}",
            "args": [
                {"arg": "arg1", "default": "default1"},
                {"arg": "arg2", "default": "default2"},
                {"arg": "arg3", "default": "default3"},
            ],
        }
    }

    manifest_path.write_text(tomli_w.dumps(manifest_content))

    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest_path, "task_with_defaults"],
        stdout_contains="Running task with default1 and default2 and default3",
    )

    verify_cli_command(
        [
            pixi,
            "run",
            "--manifest-path",
            manifest_path,
            "task_with_defaults",
            "custom1",
            "custom2",
        ],
        stdout_contains="Running task with custom1 and custom2 and default3",
    )

    verify_cli_command(
        [
            pixi,
            "run",
            "--manifest-path",
            manifest_path,
            "task_with_defaults",
            "custom1",
            "custom2",
            "custom3",
        ],
        stdout_contains="Running task with custom1 and custom2 and custom3",
    )


def test_task_args_with_some_defaults(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Test tasks with a mix of required and default arguments."""
    manifest_path = tmp_pixi_workspace.joinpath("pixi.toml")

    manifest_content = tomli.loads(EMPTY_BOILERPLATE_PROJECT)

    manifest_content["tasks"] = {
        "task_with_some_defaults": {
            "cmd": "echo Testing {{ required_arg }} with {{ optional_arg }}",
            "args": [
                "required_arg",
                {"arg": "optional_arg", "default": "optional-default"},
            ],
        }
    }

    manifest_path.write_text(tomli_w.dumps(manifest_content))

    verify_cli_command(
        [
            pixi,
            "run",
            "--manifest-path",
            manifest_path,
            "task_with_some_defaults",
            "required-value",
        ],
        stdout_contains="Testing required-value with optional-default",
    )

    verify_cli_command(
        [
            pixi,
            "run",
            "--manifest-path",
            manifest_path,
            "task_with_some_defaults",
            "required-value",
            "custom-optional",
        ],
        stdout_contains="Testing required-value with custom-optional",
    )


def test_task_args_all_required(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Test tasks where all arguments are required."""
    manifest_path = tmp_pixi_workspace.joinpath("pixi.toml")

    manifest_content = tomli.loads(EMPTY_BOILERPLATE_PROJECT)

    manifest_content["tasks"] = {
        "task_all_required": {
            "cmd": "echo All args required: {{ arg1 }} {{ arg2 }} {{ arg3 }}",
            "args": ["arg1", "arg2", "arg3"],
        }
    }

    manifest_path.write_text(tomli_w.dumps(manifest_content))

    verify_cli_command(
        [
            pixi,
            "run",
            "--manifest-path",
            manifest_path,
            "task_all_required",
            "val1",
            "val2",
            "val3",
        ],
        stdout_contains="All args required: val1 val2 val3",
    )

    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest_path, "task_all_required", "val1"],
        ExitCode.FAILURE,
        stderr_contains="no value provided for argument arg2 in task task_all_required",
    )


def test_task_args_too_many(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Test error handling when too many arguments are provided."""
    manifest_path = tmp_pixi_workspace.joinpath("pixi.toml")

    manifest_content = tomli.loads(EMPTY_BOILERPLATE_PROJECT)

    manifest_content["tasks"] = {
        "task_with_defaults": {
            "cmd": "echo Running task with {{ arg1 }} and {{ arg2 }} and {{ arg3 }}",
            "args": [
                {"arg": "arg1", "default": "default1"},
                {"arg": "arg2", "default": "default2"},
                {"arg": "arg3", "default": "default3"},
            ],
        }
    }

    manifest_path.write_text(tomli_w.dumps(manifest_content))

    verify_cli_command(
        [
            pixi,
            "run",
            "--manifest-path",
            manifest_path,
            "task_with_defaults",
            "a",
            "b",
            "c",
            "d",
        ],
        ExitCode.FAILURE,
        stderr_contains="task task_with_defaults has more arguments than expected",
    )


def test_task_with_dependency_args(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Test passing arguments to a dependency task."""
    manifest_path = tmp_pixi_workspace.joinpath("pixi.toml")

    manifest_content = tomli.loads(EMPTY_BOILERPLATE_PROJECT)

    manifest_content["tasks"] = {
        "base-task": {
            "cmd": "echo Base task with {{ arg1 }} and {{ arg2 }}",
            "args": [
                {"arg": "arg1", "default": "default1"},
                {"arg": "arg2", "default": "default2"},
            ],
        },
        "parent-task": {"depends-on": [{"task": "base-task", "args": ["custom1", "custom2"]}]},
        "parent-task-partial": {"depends-on": [{"task": "base-task", "args": ["override1"]}]},
    }

    manifest_path.write_text(tomli_w.dumps(manifest_content))

    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest_path, "parent-task"],
        stdout_contains="Base task with custom1 and custom2",
    )

    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest_path, "parent-task-partial"],
        stdout_contains="Base task with override1 and default2",
    )


def test_complex_task_dependencies_with_args(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Test complex task dependencies with arguments."""
    manifest_path = tmp_pixi_workspace.joinpath("pixi.toml")

    manifest_content = tomli.loads(EMPTY_BOILERPLATE_PROJECT)

    manifest_content["tasks"] = {
        "install": {
            "cmd": "echo Installing with manifest {{ path }} and flag {{ flag }}",
            "args": [
                {"arg": "path", "default": "/default/path"},
                {"arg": "flag", "default": "--normal"},
            ],
        },
        "build": {"cmd": "echo Building with {{ mode }}", "args": ["mode"]},
        "install-release": {
            "depends-on": [{"task": "install", "args": ["/path/to/manifest", "--debug"]}]
        },
        "deploy": {
            "cmd": "echo Deploying",
            "depends-on": [
                {"task": "install", "args": ["/custom/path", "--verbose"]},
                {"task": "build", "args": ["production"]},
            ],
        },
    }

    manifest_path.write_text(tomli_w.dumps(manifest_content))

    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest_path, "install-release"],
        stdout_contains="Installing with manifest /path/to/manifest and flag --debug",
    )

    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest_path, "deploy"],
        stdout_contains=[
            "Installing with manifest /custom/path and flag --verbose",
            "Building with production",
            "Deploying",
        ],
    )


def test_depends_on_with_complex_args(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Test task dependencies with complex argument handling."""
    manifest_path = tmp_pixi_workspace.joinpath("pixi.toml")

    manifest_content = tomli.loads(EMPTY_BOILERPLATE_PROJECT)

    manifest_content["tasks"] = {
        "helper-task": {
            "cmd": "echo Helper executed with mode={{ mode }} and level={{ level }}",
            "args": [
                {"arg": "mode", "default": "normal"},
                {"arg": "level", "default": "info"},
            ],
        },
        "utility-task": {
            "cmd": "echo Utility with arg={{ required_arg }}",
            "args": ["required_arg"],
        },
        "main-task": {
            "cmd": "echo Main task executed",
            "depends-on": [
                {"task": "helper-task", "args": ["debug", "verbose"]},
                {"task": "utility-task", "args": ["important-data"]},
            ],
        },
        "partial-args-task": {
            "cmd": "echo Partial args task",
            "depends-on": [
                {
                    "task": "helper-task",
                    "args": ["production"],
                }
            ],
        },
        "mixed-dependency-types": {
            "cmd": "echo Mixed dependencies",
            "depends-on": [
                "utility-task",
                {"task": "helper-task"},
            ],
        },
    }

    manifest_path.write_text(tomli_w.dumps(manifest_content))

    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest_path, "main-task"],
        stdout_contains=[
            "Helper executed with mode=debug and level=verbose",
            "Utility with arg=important-data",
            "Main task executed",
        ],
    )

    verify_cli_command(
        [pixi, "run", "--manifest-path", manifest_path, "partial-args-task"],
        stdout_contains=[
            "Helper executed with mode=production and level=info",
            "Partial args task",
        ],
    )

    verify_cli_command(
        [
            pixi,
            "run",
            "--manifest-path",
            manifest_path,
            "mixed-dependency-types",
            "some-arg",
        ],
        ExitCode.FAILURE,
        stderr_contains="no value provided for argument required_arg in task utility-task",
    )
