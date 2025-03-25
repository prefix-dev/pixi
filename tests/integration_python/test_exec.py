from pathlib import Path
import sys

import pytest
from .common import verify_cli_command
from concurrent.futures import ProcessPoolExecutor, as_completed


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
