import json
import re
import signal
import subprocess
import sys
import time
from pathlib import Path

import pytest

from .common import CONDA_FORGE_CHANNEL, CURRENT_PLATFORM, ExitCode, verify_cli_command

PYTHON_ARGV_SCRIPT = f"""# /// conda-script
# channels = ["{CONDA_FORGE_CHANNEL}"]
# entrypoint = "python ${{SCRIPT}}"
#
# [dependencies]
# python = "3.13.*"
# /// end-conda-script
import json, os, sys

print(json.dumps({{"argv": sys.argv[1:], "cwd": os.getcwd(), "file": __file__}}))
"""


def json_payload(stdout: str) -> dict[str, object]:
    return json.loads(next(line for line in stdout.splitlines() if line.startswith("{")))


def test_run_requires_the_experimental_flag(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "example.code"
    script.write_text(PYTHON_ARGV_SCRIPT)

    verify_cli_command(
        [pixi, "run", "--script", script],
        ExitCode.FAILURE,
        stderr_contains=["conda-script block", "--experimental"],
    )


def test_the_config_option_replaces_the_experimental_flag(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    script = tmp_pixi_workspace / "example.code"
    script.write_text(
        "# /// conda-script\n"
        '# channels = ["conda-forge"]\n'
        '# entrypoint = "python ${SCRIPT} | tee log"\n'
        "# /// end-conda-script\n"
    )
    config = tmp_pixi_workspace / ".pixi" / "config.toml"
    config.write_text(
        config.read_text().replace("[experimental]\n", "[experimental]\nconda-script = true\n")
    )

    # The entrypoint is rejected, so the run got past the experimental gate.
    verify_cli_command(
        [pixi, "run", "--script", script, "--dry-run"],
        ExitCode.FAILURE,
        stderr_contains=["experimental.conda-script", "pipes are not supported"],
    )


def test_experimental_requires_a_script(pixi: Path) -> None:
    verify_cli_command(
        [pixi, "run", "--experimental", "task"],
        ExitCode.INCORRECT_USAGE,
        stderr_contains="--script",
    )


def test_rejects_a_file_with_both_block_kinds(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "example.py"
    script.write_text(
        "# /// script\n"
        "# dependencies = []\n"
        "# ///\n"
        "# /// conda-script\n"
        '# channels = ["conda-forge"]\n'
        '# entrypoint = "python ${SCRIPT}"\n'
        "# /// end-conda-script\n"
    )

    verify_cli_command(
        [pixi, "run", "--experimental", "--script", script],
        ExitCode.FAILURE,
        stderr_contains="both a PEP 723",
    )


def test_pep723_routing_is_unchanged(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "example.py"
    script.write_text("print('hello')\n")

    # A Python file without any block reports the PEP 723 error, with and
    # without the flag, which has no effect for them.
    for command in (
        [pixi, "run", "--script", script],
        [pixi, "run", "--experimental", "--script", script],
    ):
        verify_cli_command(
            command,
            ExitCode.FAILURE,
            stderr_contains="does not contain a PEP 723 metadata block",
        )


def test_a_stray_marker_in_a_python_file_stays_on_the_pep723_path(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    script = tmp_pixi_workspace / "example.py"
    script.write_text('"""\n    # /// conda-script\n"""\nprint()\n')

    # Without the flag the malformed pseudo-block must not break the file.
    verify_cli_command(
        [pixi, "run", "--script", script],
        ExitCode.FAILURE,
        stderr_contains="does not contain a PEP 723 metadata block",
    )
    # With the flag the block error surfaces.
    verify_cli_command(
        [pixi, "run", "--experimental", "--script", script],
        ExitCode.FAILURE,
        stderr_contains="no closing",
    )


def test_init_refuses_a_conda_script_file(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "example.py"
    contents = (
        "# /// conda-script\n"
        '# channels = ["conda-forge"]\n'
        '# entrypoint = "python ${SCRIPT}"\n'
        "# /// end-conda-script\n"
    )
    script.write_text(contents)

    # Prepending a PEP 723 block would leave a file with both kinds that no
    # command accepts anymore.
    verify_cli_command(
        [pixi, "init", "--script", script],
        ExitCode.FAILURE,
        stderr_contains="already a conda-script",
    )
    assert script.read_text() == contents


def test_stdin_reports_conda_script_blocks(pixi: Path) -> None:
    verify_cli_command(
        [pixi, "run", "--script", "-"],
        ExitCode.FAILURE,
        stderr_contains="not supported on stdin",
        stdin=(
            "# /// conda-script\n"
            '# channels = ["conda-forge"]\n'
            '# entrypoint = "python ${SCRIPT}"\n'
            "# /// end-conda-script\n"
        ),
    )


def test_entrypoint_syntax_errors_are_reported(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "example.code"
    script.write_text(
        "# /// conda-script\n"
        '# channels = ["conda-forge"]\n'
        '# entrypoint = "python ${SCRIPT} | tee log"\n'
        "# /// end-conda-script\n"
    )

    verify_cli_command(
        [pixi, "run", "--experimental", "--script", script, "--dry-run"],
        ExitCode.FAILURE,
        stderr_contains="pipes are not supported",
    )


def test_requires_pixi_is_enforced(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "requires-pixi.code"
    script.write_text(f"""# /// conda-script
# channels = ["{CONDA_FORGE_CHANNEL}"]
# entrypoint = "echo ok"
#
# [tool.pixi.workspace]
# requires-pixi = ">=999"
# /// end-conda-script
""")

    verify_cli_command(
        [pixi, "workspace", "platform", "list", "--script", script, "--json"],
        ExitCode.FAILURE,
        stderr_contains=[
            "this project requires pixi '>=999'",
            "this version requirement is not satisfied",
        ],
    )


def test_implicit_platform_accepts_matching_target(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "target.code"
    script.write_text(f"""# /// conda-script
# channels = ["{CONDA_FORGE_CHANNEL}"]
# entrypoint = "echo ok"
#
# [tool.pixi.target.{CURRENT_PLATFORM}.dependencies]
# zlib = "*"
# /// end-conda-script
""")

    verify_cli_command(
        [pixi, "workspace", "platform", "list", "--script", script, "--json"],
        stderr_excludes="does not match any of the platforms supported by the workspace",
    )


@pytest.mark.slow
@pytest.mark.skipif(sys.platform == "win32", reason="signals are a Unix concept")
def test_a_signal_sent_to_pixi_reaches_the_entrypoint(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """A SIGTERM delivered to pixi itself, say by a supervisor, is forwarded
    to the entrypoint's process instead of orphaning it."""
    script = tmp_pixi_workspace / "signal.code"
    started = tmp_pixi_workspace / "started"
    terminated = tmp_pixi_workspace / "terminated"
    script.write_text(f"""# /// conda-script
# channels = ["{CONDA_FORGE_CHANNEL}"]
# entrypoint = "sh -c \\"trap 'touch {terminated}' TERM; touch {started}; sleep 30 & wait\\""
#
# [dependencies]
# coreutils = "*"
# /// end-conda-script
""")

    process = subprocess.Popen([pixi, "run", "--experimental", "--script", script])
    try:
        deadline = time.monotonic() + 600
        while not started.exists():
            assert process.poll() is None, "pixi exited before the entrypoint started"
            assert time.monotonic() < deadline, "the entrypoint never started"
            time.sleep(0.2)

        process.send_signal(signal.SIGTERM)
        process.wait(timeout=30)
    finally:
        if process.poll() is None:
            process.kill()

    deadline = time.monotonic() + 10
    while not terminated.exists() and time.monotonic() < deadline:
        time.sleep(0.1)
    assert terminated.exists(), "the entrypoint's process never received the forwarded SIGTERM"


@pytest.mark.slow
def test_arguments_and_working_directory_reach_the_entrypoint(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    script = tmp_pixi_workspace / "scripts" / "example.code"
    script.parent.mkdir()
    script.write_text(PYTHON_ARGV_SCRIPT)
    invocation_dir = tmp_pixi_workspace / "elsewhere"
    invocation_dir.mkdir()

    output = verify_cli_command(
        [pixi, "run", "--experimental", "--script", script, "first", "--second"],
        cwd=invocation_dir,
    )

    payload = json_payload(output.stdout)
    assert payload["argv"] == ["first", "--second"]
    assert payload["cwd"] == str(invocation_dir)
    assert payload["file"] == str(script)
    assert not (script.parent / "example.code.pixi.lock").exists()


@pytest.mark.slow
def test_the_cache_directory_persists_between_runs(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "counter.code"
    script.write_text(f"""# /// conda-script
# channels = ["{CONDA_FORGE_CHANNEL}"]
# entrypoint = "python ${{SCRIPT}} ${{CACHE}}"
#
# [dependencies]
# python = "3.13.*"
# /// end-conda-script
import pathlib, sys

counter = pathlib.Path(sys.argv[1]) / "counter"
runs = int(counter.read_text()) + 1 if counter.exists() else 1
counter.write_text(str(runs))
print(f"run {{runs}}")
""")

    # The cache is keyed by the absolute script path, which a reused pytest
    # temp directory repeats, so assert on the increment rather than on
    # absolute counts.
    first = verify_cli_command([pixi, "run", "--experimental", "--script", script])
    count = re.search(r"run (\d+)", first.stdout)
    assert count is not None
    runs = int(count.group(1))
    verify_cli_command(
        [pixi, "run", "--experimental", "--script", script],
        stdout_contains=f"run {runs + 1}",
    )


@pytest.mark.slow
def test_a_failing_entrypoint_propagates_its_exit_code(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    script = tmp_pixi_workspace / "fails.code"
    script.write_text(f"""# /// conda-script
# channels = ["{CONDA_FORGE_CHANNEL}"]
# entrypoint = "python ${{SCRIPT}} && python -c 'open(\\"marker\\", \\"w\\").close()'"
#
# [dependencies]
# python = "3.13.*"
# /// end-conda-script
import sys

print("about to fail")
sys.exit(1)
""")

    verify_cli_command(
        [pixi, "run", "--experimental", "--script", script],
        ExitCode.FAILURE,
        stdout_contains="about to fail",
        cwd=tmp_pixi_workspace,
    )
    assert not (tmp_pixi_workspace / "marker").exists()


SOURCE_DEPENDENCY_BLOCK = f"""# /// conda-script
# channels = ["{CONDA_FORGE_CHANNEL}"]
# entrypoint = "simple-app"
#
# [dependencies]
# simple-app = "{{version}}"
#
# [tool.pixi.workspace]
# preview = ["pixi-build"]
#
# [tool.pixi.dependencies]
# simple-app = {{{{ git = "https://github.com/prefix-dev/pixi-build-testsuite.git", subdirectory = "tests/data/pixi_build/minimal-backend-workspaces/pixi-build-python" }}}}
# /// end-conda-script
"""


@pytest.mark.slow
def test_a_binary_spec_constrains_a_source_dependency(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Both specs of the package reach the solver: the source spec in
    `[tool.pixi.dependencies]` provides the candidate and the binary spec in
    `[dependencies]` constrains its version. The preview declared under
    `[tool.pixi.workspace]` lets the source dependency build."""
    script = tmp_pixi_workspace / "source.code"
    script.write_text(SOURCE_DEPENDENCY_BLOCK.format(version="0.1.*"))

    verify_cli_command(
        [pixi, "run", "--experimental", "--script", script],
        stdout_contains="Build backend works",
    )

    script.write_text(SOURCE_DEPENDENCY_BLOCK.format(version="0.2.*"))
    verify_cli_command(
        [pixi, "run", "--experimental", "--script", script],
        ExitCode.FAILURE,
        stderr_contains="0.2",
    )


@pytest.mark.slow
def test_tool_pixi_tables_shape_the_environment(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """`tool.pixi` is read like a manifest: activation applies to the
    entrypoint and the workspace options reach the solver."""
    script = tmp_pixi_workspace / "tool.code"
    script.write_text(f"""# /// conda-script
# channels = ["{CONDA_FORGE_CHANNEL}"]
# entrypoint = "python -c \\"import os; print(os.environ['GREETING'])\\""
#
# [dependencies]
# python = "3.13.*"
#
# [tool.pixi.workspace]
# exclude-newer = "2030-01-01"
#
# [tool.pixi.activation.env]
# GREETING = "hello from tool.pixi"
# /// end-conda-script
""")

    verify_cli_command(
        [pixi, "run", "--experimental", "--script", script],
        stdout_contains="hello from tool.pixi",
    )


def test_run_rejects_workspace_only_tables(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "feature.code"
    script.write_text(f"""# /// conda-script
# channels = ["{CONDA_FORGE_CHANNEL}"]
# entrypoint = "python ${{SCRIPT}}"
#
# [tool.pixi.feature.test.dependencies]
# pytest = "*"
# /// end-conda-script
""")

    verify_cli_command(
        [pixi, "run", "--experimental", "--script", script],
        ExitCode.FAILURE,
        stderr_contains=[
            "scripts do not support `tool.pixi.feature`",
            "one implicit default environment",
        ],
    )


@pytest.mark.slow
def test_lock_writes_an_adjacent_lock_file_with_conditional_dependencies(
    pixi: Path, tmp_pixi_workspace: Path
) -> None:
    script = tmp_pixi_workspace / "when.code"
    script.write_text(f"""# /// conda-script
# channels = ["{CONDA_FORGE_CHANNEL}"]
# entrypoint = "python -c 'print(1234)'"
#
# [dependencies]
# python = "3.13.*"
# zlib = {{ version = "*", when = "__unix" }}
# vc = {{ version = "*", when = "__win" }}
# /// end-conda-script
""")
    lock_file = tmp_pixi_workspace / "when.code.pixi.lock"

    verify_cli_command([pixi, "lock", "--script", script])

    assert lock_file.exists()
    locked = lock_file.read_text()
    if CURRENT_PLATFORM.startswith("win"):
        assert "/vc-" in locked
        assert "/zlib-" not in locked
    else:
        assert "/zlib-" in locked
        assert "/vc-" not in locked

    # A run next to the lock file consumes it.
    verify_cli_command(
        [pixi, "run", "--experimental", "--script", script, "--frozen"],
        stdout_contains="1234",
    )


def test_add_edits_the_block_and_locks(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "tool.code"
    script.write_text(f"""# /// conda-script
# channels = ["{CONDA_FORGE_CHANNEL}"]
# entrypoint = "main"
#
# [dependencies]
# zlib = "*"
# /// end-conda-script
body
""")

    # An adjacent lock file makes the edit commands update it in place.
    verify_cli_command([pixi, "lock", "--script", script])
    lock_file = tmp_pixi_workspace / "tool.code.pixi.lock"
    assert "/xz-" not in lock_file.read_text()

    verify_cli_command([pixi, "add", "--script", script, "--no-install", "xz"])

    contents = script.read_text()
    assert re.search(r"^# xz = \">=", contents, re.MULTILINE)
    # The code around the block stays untouched.
    assert contents.endswith("body\n")
    assert "/xz-" in lock_file.read_text()

    verify_cli_command(
        [pixi, "list", "--script", script, "--no-install"],
        stdout_contains=["xz", "zlib"],
    )
    verify_cli_command(
        [pixi, "tree", "--script", script, "--no-install"],
        stdout_contains="zlib",
    )
    verify_cli_command([pixi, "update", "--script", script, "--no-install"])


def test_add_pypi_writes_the_tool_pixi_table(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "pypi.code"
    script.write_text(f"""# /// conda-script
# channels = ["{CONDA_FORGE_CHANNEL}"]
# entrypoint = "python ${{SCRIPT}}"
#
# [dependencies]
# python = "3.13.*"
# /// end-conda-script
print("hi")
""")

    verify_cli_command([pixi, "add", "--script", script, "--no-install", "--pypi", "six"])

    contents = script.read_text()
    assert "# [tool.pixi.pypi-dependencies]" in contents
    assert re.search(r"^# six = ", contents, re.MULTILINE)


def test_add_rejects_options_outside_the_block_schema(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "guards.code"
    script.write_text(f"""# /// conda-script
# channels = ["{CONDA_FORGE_CHANNEL}"]
# entrypoint = "main"
# /// end-conda-script
""")

    verify_cli_command(
        [pixi, "add", "--script", script, "--platform", "linux-64", "zlib"],
        ExitCode.FAILURE,
        stderr_contains="`when` condition",
    )
    verify_cli_command(
        [
            pixi,
            "add",
            "--script",
            script,
            "--git",
            "https://github.com/prefix-dev/pixi-build-testsuite.git",
            "simple-app",
        ],
        ExitCode.FAILURE,
        stderr_contains="tool.pixi.dependencies",
    )


def test_add_rejects_a_file_with_both_block_kinds(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "both.py"
    script.write_text(
        "# /// script\n"
        "# dependencies = []\n"
        "# ///\n"
        "# /// conda-script\n"
        '# channels = ["conda-forge"]\n'
        '# entrypoint = "python ${SCRIPT}"\n'
        "# /// end-conda-script\n"
    )

    verify_cli_command(
        [pixi, "add", "--script", script, "numpy"],
        ExitCode.FAILURE,
        stderr_contains="both a PEP 723",
    )


def test_a_blockless_file_reports_the_missing_block(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "plain.sh"
    script.write_text("echo plain\n")

    verify_cli_command(
        [pixi, "list", "--script", script],
        ExitCode.FAILURE,
        stderr_contains="does not contain a conda-script block",
    )


def test_add_then_remove_pypi_round_trips(pixi: Path, tmp_pixi_workspace: Path) -> None:
    script = tmp_pixi_workspace / "roundtrip.code"
    script.write_text(f"""# /// conda-script
# channels = ["{CONDA_FORGE_CHANNEL}"]
# entrypoint = "python ${{SCRIPT}}"
#
# [dependencies]
# python = "3.13.*"
# /// end-conda-script
""")

    verify_cli_command([pixi, "add", "--script", script, "--no-install", "--pypi", "six"])
    assert "six" in script.read_text()

    verify_cli_command([pixi, "remove", "--script", script, "--no-install", "--pypi", "six"])
    assert "six" not in script.read_text()


def test_remove_edits_the_block_dependencies(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """The block's `[dependencies]` are the default feature, which is where
    `pixi remove` looks for a conda dependency."""
    script = tmp_pixi_workspace / "remove.code"
    script.write_text(f"""# /// conda-script
# channels = ["{CONDA_FORGE_CHANNEL}"]
# entrypoint = "python ${{SCRIPT}}"
#
# [dependencies]
# python = "3.13.*"
# zlib = "*"
#
# [tool.pixi.activation.env]
# GREETING = "kept"
# /// end-conda-script
""")

    verify_cli_command([pixi, "remove", "--script", script, "--no-install", "zlib"])

    contents = script.read_text()
    assert "zlib" not in contents
    assert '# python = "3.13.*"' in contents
    assert '# GREETING = "kept"' in contents
