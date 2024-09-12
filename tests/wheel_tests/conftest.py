from generate_summaries import terminal_summary, markdown_summary
from helpers import setup_stdout_stderr_logging


def pytest_configure(config):
    setup_stdout_stderr_logging()


def pytest_addoption(parser):
    parser.addoption(
        "--pixi-exec", action="store", default="pixi", help="Path to the pixi executable"
    )


def pytest_terminal_summary(terminalreporter, exitstatus, config):
    """
    At the end of the test session, generate a summary report.
    """
    terminal_summary()


def pytest_sessionfinish(session, exitstatus):
    """
    At the end of the test session, generate a `.summary.md` report. That contains the
    same information as the terminal summary.
    """
    markdown_summary()
