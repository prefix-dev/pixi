import shutil
import tempfile
import sys
from pathlib import Path
from subprocess import run, PIPE

import pytest

from .common import (
    binary_github_release_url,
    verify_cli_command
)

# A different older version to downgrade to, for a genuine test scenario.
TARGET_VERSION = "0.59.0"

def test_self_update_from_url(pixi: Path, tmp_path: Path):
    """Test pixi self-update from a direct URL."""
    # Copy the running pixi binary to a temp location for testing. This is the
    # file that will be rewritten by the self-update.
    pixi_copy = tmp_path / "pixi.exe"
    shutil.copy(pixi, pixi_copy)
    pixi_copy.chmod(0o755)

    url = binary_github_release_url(TARGET_VERSION)

    # The current version should be different from TARGET_VERSION, for this
    # test to be meaningful.
    verify_cli_command(
        [pixi_copy, "--version"],
        stdout_excludes=[TARGET_VERSION],
    )

    verify_cli_command(
        [pixi_copy,"self-update", "--url", url],
        stdout_contains=[TARGET_VERSION],
    )

    verify_cli_command(
        [pixi_copy, "--version"],
        stdout_contains=[TARGET_VERSION],
    )
