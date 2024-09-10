import sys
import json
from rich.console import Console
from rich.table import Table
from rich.text import Text


def pytest_addoption(parser):
    if sys.platform.startswith("win"):
        parser.addoption(
            "--pixi-exec", action="store", default="pixi.exe", help="Path to the pixi executable"
        )
    else:
        parser.addoption(
            "--pixi-exec", action="store", default="pixi", help="Path to the pixi executable"
        )


RESULTS_FILE = "test_results.json"
# def pytest_configure(config):
#     """
#     Initializes the test results file.
#     """
#     # Delete the file if it exists and create a new empty file
#     results = Path(RESULTS_FILE)
#     if results.exists():
#         results.unlink()
#     with results.open("w") as f:
#         json.dump([], f)


def pytest_terminal_summary(terminalreporter, exitstatus, config):
    """
    At the end of the test session, generate a summary report.
    """
    # Read aggregated results from the shared file
    with open(RESULTS_FILE, "r") as f:
        results = json.load(f)

    console = Console()
    table = Table(title="Test Results", show_header=True, header_style="bold magenta")
    table.add_column("Test Name", style="dim", width=60)
    table.add_column("Outcome", justify="right")
    table.add_column("Duration (s)", justify="right")
    table.add_column("Error Details")

    # Populate the table with collected results
    for result in results:
        outcome_color = "green" if result["outcome"] == "passed" else "red"
        error_details = result["longrepr"] if result["outcome"] == "failed" else ""
        table.add_row(
            result["name"],
            Text(result["outcome"], style=outcome_color),
            f"{result['duration']:.2f}",
            error_details,
        )

    # Display the table in the terminal
    console.print(table)
