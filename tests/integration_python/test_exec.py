import platform
import sys
from concurrent.futures import ProcessPoolExecutor, as_completed
from pathlib import Path

import pytest

from .common import ExitCode, run_and_get_env, verify_cli_command


@pytest.mark.skipif(
    sys.platform.startswith("win"),
    reason="For some reason .bat files are not correctly executed on windows",
)
def test_concurrent_exec(pixi: Path, dummy_channel_1: str) -> None:
    with ProcessPoolExecutor(max_workers=2) as executor:
        # Run the two exact same tasks in parallel
        futures = [
            executor.submit(
                verify_cli_command,
                [pixi, "exec", "-c", dummy_channel_1, "dummy-f"],
                stdout_contains=["dummy-f on"],
            ),
            executor.submit(
                verify_cli_command,
                [pixi, "exec", "-c", dummy_channel_1, "dummy-f"],
                stdout_contains=["dummy-f on"],
            ),
        ]

        # Ensure both tasks are actually running in parallel and wait for them to finish
        for future in as_completed(futures):
            future.result()


@pytest.mark.skipif(
    sys.platform.startswith("win"),
    reason="For some reason .bat files are not correctly executed on windows",
)
def test_exec_list(pixi: Path, dummy_channel_1: str) -> None:
    # Without `--list`, nothing is listed
    verify_cli_command(
        [pixi, "exec", "--channel", dummy_channel_1, "dummy-g"],
        stdout_excludes=["dummy-g"],
    )

    # List all packages in environment
    verify_cli_command(
        [pixi, "exec", "--channel", dummy_channel_1, "--list", "dummy-g"],
        stdout_contains=["dummy-g", "dummy-b"],
    )

    # List only packages that match regex "g"
    verify_cli_command(
        [pixi, "exec", "--channel", dummy_channel_1, "--list=g", "dummy-g"],
        stdout_contains="dummy-g",
        stdout_excludes="dummy-b",
    )

    # List specific package
    verify_cli_command(
        [pixi, "exec", "--channel", dummy_channel_1, "--list=dummy-g", "dummy-g"],
        stdout_contains=["dummy-g"],
        stdout_excludes=["dummy-b"],
    )


def test_pixi_environment_name_and_ps1(pixi: Path, dummy_channel_1: str) -> None:
    """Test that PIXI_ENVIRONMENT_NAME and PS1/PROMPT are set correctly."""
    # Test with single package
    env_value, _ = run_and_get_env(
        pixi, "--channel", dummy_channel_1, "-s", "dummy-a", env_var="PIXI_ENVIRONMENT_NAME"
    )
    assert env_value == "temp:dummy-a"

    # Test with multiple packages (should be sorted)
    env_value, _ = run_and_get_env(
        pixi,
        "--channel",
        dummy_channel_1,
        "-s",
        "dummy-c",
        "-s",
        "dummy-a",
        env_var="PIXI_ENVIRONMENT_NAME",
    )
    assert env_value == "temp:dummy-a,dummy-c"

    # Test with --with flag
    env_value, _ = run_and_get_env(
        pixi,
        "--channel",
        dummy_channel_1,
        "--with",
        "dummy-b",
        "--with",
        "dummy-c",
        env_var="PIXI_ENVIRONMENT_NAME",
    )
    assert env_value == "temp:dummy-b,dummy-c"

    # Test with no specs (should not set the variable)
    env_value, _ = run_and_get_env(
        pixi, "--channel", dummy_channel_1, env_var="PIXI_ENVIRONMENT_NAME"
    )
    assert env_value is None

    # Test PS1 modification
    if sys.platform.startswith("win"):
        prompt_var = "_PIXI_PROMPT"
        expected_prompt = "(pixi:temp:dummy-a) $P$G"
    else:
        prompt_var = "PS1"
        expected_prompt = r"(pixi:temp:dummy-a) [\w] \$"

    # Test with default behavior (prompt should be modified)
    prompt, _ = run_and_get_env(
        pixi, "--channel", dummy_channel_1, "-s", "dummy-a", env_var=prompt_var
    )
    assert prompt == expected_prompt

    # Test with --no-modify-ps1 (prompt should not be modified)
    prompt, _ = run_and_get_env(
        pixi,
        "--channel",
        dummy_channel_1,
        "--no-modify-ps1",
        "-s",
        "dummy-a",
        env_var=prompt_var,
    )
    if sys.platform.startswith("win"):
        assert prompt is None
    else:
        assert prompt is None or "(pixi:temp:dummy-a)" not in prompt


@pytest.mark.skipif(
    sys.platform.startswith("win"),
    reason="For some reason .bat files are not correctly executed on windows",
)
def test_exec_with(pixi: Path, dummy_channel_1: str) -> None:
    # A package is guessed from the command when `--with` is provided
    verify_cli_command(
        [pixi, "exec", "--channel", dummy_channel_1, "--list", "--spec=dummy-a", "dummy-f"],
        stdout_excludes="dummy-f",
        expected_exit_code=ExitCode.FAILURE,
    )
    verify_cli_command(
        [pixi, "exec", "--channel", dummy_channel_1, "--list", "--with=dummy-a", "dummy-f"],
        stdout_contains="dummy-f",
    )

    # Correct behaviour with multiple 'with' options
    verify_cli_command(
        [
            pixi,
            "exec",
            "--channel",
            dummy_channel_1,
            "--list",
            "--with=dummy-a",
            "--with=dummy-b",
            "dummy-f",
        ],
        stdout_contains=["dummy-a", "dummy-b", "dummy-f"],
    )

    # 'with' and 'spec' options mutually exclusive
    verify_cli_command(
        [
            pixi,
            "exec",
            "--channel",
            dummy_channel_1,
            "--list",
            "--with=dummy-a",
            "--spec=dummy-b",
            "dummy-f",
        ],
        expected_exit_code=ExitCode.INCORRECT_USAGE,
        stderr_contains="cannot be used with",
    )


@pytest.mark.slow
def test_conda_script_python(pixi: Path, tmp_path: Path) -> None:
    """Test conda-script metadata with Python script (basic e2e test)."""
    script = tmp_path / "test_script.py"
    script.write_text(
        """#!/usr/bin/env python
# /// conda-script
# [dependencies]
# python = "3.12.*"
# [script]
# channels = ["conda-forge"]
# entrypoint = "python"
# /// end-conda-script

print("Hello from conda-script!")
"""
    )

    verify_cli_command(
        [pixi, "exec", str(script)],
        stdout_contains="Hello from conda-script!",
    )


def test_conda_script_with_cli_specs(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    """Test that CLI specs override conda-script metadata."""
    script = tmp_path / "test_script.py"
    script.write_text(
        """#!/usr/bin/env python
# /// conda-script
# [dependencies]
# python = "3.12.*"
# [script]
# channels = ["conda-forge"]
# /// end-conda-script

print("Script executed")
"""
    )

    # CLI specs should override script metadata
    verify_cli_command(
        [pixi, "exec", "--channel", dummy_channel_1, "-s", "dummy-a", str(script)],
        stdout_contains="Script executed",
    )


def test_conda_script_no_metadata(pixi: Path, tmp_path: Path, dummy_channel_1: str) -> None:
    """Test that scripts without metadata still work."""
    script = tmp_path / "test_script.py"
    script.write_text(
        """#!/usr/bin/env python
print("Hello without metadata!")
"""
    )

    # Should work with regular exec behavior
    verify_cli_command(
        [pixi, "exec", "--channel", dummy_channel_1, "-s", "python", str(script)],
        stdout_contains="Hello without metadata!",
    )


def test_exec_with_relative_path(
    pixi: Path, dummy_channel_1: str, test_data: Path, tmp_path: Path
) -> None:
    artifact = _dummy_artifact(test_data)
    cwd = Path.cwd()
    try:
        relative_path = artifact.relative_to(cwd)
        spec_value = f"./{relative_path}"
    except ValueError:
        spec_value = str(artifact)

    cache_dir = tmp_path / "pixi-cache"
    cache_dir.mkdir(parents=True, exist_ok=True)
    env = {"PIXI_CACHE_DIR": str(cache_dir)}
    expected_env = f"PIXI_ENVIRONMENT_NAME=temp:{artifact.name}"
    verify_cli_command(
        [pixi, "exec", f"--channel={dummy_channel_1}", "--spec", spec_value, "env"],
        stdout_contains=[expected_env],
        env=env,
    )


def test_exec_with_absolute_path(
    pixi: Path, dummy_channel_1: str, test_data: Path, tmp_path: Path
) -> None:
    artifact = _dummy_artifact(test_data)
    cache_dir = tmp_path / "pixi-cache"
    cache_dir.mkdir(parents=True, exist_ok=True)
    env = {"PIXI_CACHE_DIR": str(cache_dir)}
    expected_env = "PIXI_ENVIRONMENT_NAME=temp:dummy-a"
    verify_cli_command(
        [pixi, "exec", f"--channel={dummy_channel_1}", "--spec", str(artifact), "env"],
        stdout_contains=[expected_env],
        env=env,
    )


def test_exec_with_url(pixi: Path, dummy_channel_1: str, tmp_path: Path) -> None:
    cache_dir = tmp_path / "pixi-cache"
    cache_dir.mkdir(parents=True, exist_ok=True)
    env = {"PIXI_CACHE_DIR": str(cache_dir)}
    # Test with HTTPS URL (file:// URLs are not supported as they're not recognized by rattler)
    # For local files, use absolute or relative paths instead
    verify_cli_command(
        [
            pixi,
            "exec",
            f"--channel={dummy_channel_1}",
            "--spec",
            "https://conda.anaconda.org/conda-forge/noarch/tzdata-2024b-hc8b5060_0.conda",
            "env",
        ],
        stdout_contains=["PIXI_ENVIRONMENT_NAME=temp:tzdata"],
        env=env,
    )


def _dummy_artifact(test_data: Path) -> Path:
    if sys.platform.startswith("linux"):
        return (
            test_data
            / "channels"
            / "channels"
            / "dummy_channel_1"
            / "linux-64"
            / "dummy-a-0.1.0-hb0f4dca_0.conda"
        )

    if sys.platform == "darwin":
        machine = platform.machine().lower()
        if machine == "x86_64":
            subdir = "osx-64"
            build = "h0dc7051_0"
        else:
            subdir = "osx-arm64"
            build = "h60d57d3_0"
        return (
            test_data
            / "channels"
            / "channels"
            / "dummy_channel_1"
            / subdir
            / f"dummy-a-0.1.0-{build}.conda"
        )

    if sys.platform.startswith("win"):
        return (
            test_data
            / "channels"
            / "channels"
            / "dummy_channel_1"
            / "win-64"
            / "dummy-a-0.1.0-h9490d1a_0.conda"
        )

    pytest.skip("exec path tests not supported on this platform")
