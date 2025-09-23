# pyright: reportUnusedParameter=false

from pathlib import Path
from typing import Any

import pytest

from .helpers import setup_stdout_stderr_logging
from .generate_summaries import terminal_summary, markdown_summary


def pytest_configure(config: pytest.Config) -> None:
    setup_stdout_stderr_logging()


def pytest_addoption(parser: pytest.Parser) -> None:
    # Used to override the default path to the pixi executable
    parser.addoption("--pixi-exec", action="store", help="Path to the pixi executable")


def pytest_terminal_summary(terminalreporter: Any, exitstatus: int, config: pytest.Config) -> None:
    """
    At the end of the test session, generate a summary report.
    """
    terminal_summary()


def pytest_sessionfinish(session: pytest.Session, exitstatus: int) -> None:
    """
    At the end of the test session, generate a `.summary.md` report. That contains the
    same information as the terminal summary.
    """
    markdown_summary()


@pytest.fixture
def tmp_pixi_workspace(tmp_path: Path) -> Path:
    pixi_config = """
[repodata-config."https://prefix.dev/"]
disable-sharded = false
"""
    tmp_path.joinpath("config.toml").write_text(pixi_config)
    return tmp_path
