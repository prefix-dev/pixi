import json
import re
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
    # without the inert flag.
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
# [tool.pixi.dependencies]
# simple-app = {{{{ git = "https://github.com/prefix-dev/pixi-build-testsuite.git", subdirectory = "tests/data/pixi_build/minimal-backend-workspaces/pixi-build-python" }}}}
# /// end-conda-script
"""


@pytest.mark.slow
def test_a_binary_spec_constrains_a_source_dependency(pixi: Path, tmp_pixi_workspace: Path) -> None:
    """Both features' specs reach the solver as a union: the source spec in
    `[tool.pixi.dependencies]` provides the candidate and the binary spec in
    `[dependencies]` constrains its version."""
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
