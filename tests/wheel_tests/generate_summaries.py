import tomllib

from pathlib import Path
from rich.console import Console
from rich.table import Table
from rich.text import Text
from rich.panel import Panel

from .read_wheels import read_wheel_file
from .record_results import RESULTS_FILE


def terminal_summary() -> None:
    # Read aggregated results from the shared file
    results_file = RESULTS_FILE
    if not results_file.exists():
        print("Error: No test results found.")
        return

    with results_file.open("rb") as f:
        results = tomllib.load(f)["results"]

    packages = read_wheel_file()

    console = Console()
    table = Table(title="Test Results", show_header=True, header_style="bold magenta")
    table.add_column("Test Name", style="dim")
    table.add_column("Outcome", justify="right")
    table.add_column("Duration (s)", justify="right")
    table.add_column("Error Details")

    # Populate the table with collected results
    names: list[str] = []
    for result in sorted(results, key=lambda r: r["name"]):
        outcome_color = "green" if result["outcome"] == "passed" else "red"
        error_details = result["longrepr"] if result["outcome"] == "failed" else ""
        table.add_row(
            Text(result["name"]),
            Text(result["outcome"], style=outcome_color),
            f"{result['duration']:.2f}",
            error_details,
        )
        # Record name
        names.append(result["name"])

    for package in packages:
        if package.to_add_cmd() not in names:
            table.add_row(
                Text(package.to_add_cmd()),
                Text("N/A", style="dim"),
                Text("N/A", style="dim"),
                Text("N/A", style="dim"),
            )

    # Display the table in the terminal
    console.print(table)

    # Add a summary box with instructions
    summary_text = (
        "[bold]Summary:[/bold]\n\n"
        f"- Total tests run: {len(results)}\n"
        f"- Passed: {sum(1 for r in results if r['outcome'] == 'passed')}\n"
        f"- Failed: {sum(1 for r in results if r['outcome'] == 'failed')}\n\n"
        "To filter tests by a specific wheel, use the command:\n"
        "[bold green]pytest -k '<pixi_add_cmd>'[/]\n\n"
        "Replace [bold]<pixi_add_com>[/] with the desired wheel's name to run only tests for that wheel.\n"
        r'E.g use [magenta] pixi r test-common-wheels-dev -k "jax\[cuda12]"[/] to run tests for the [bold]jax\[cuda12][/] wheel.'
        "\n\n"
        "[bold yellow]Note:[/]\n"
        "Any [italic]failed[/] tests will have recorded their output to the [bold].log/[/] directory, which"
        " resides next to to `wheels.toml` file.\n"
    )

    # Create a Rich panel (box) for the summary text
    summary_panel = Panel(
        summary_text, title="Test Debrief", title_align="left", border_style="bright_blue"
    )

    # Display the summary box in the terminal
    console.print(summary_panel)


def markdown_summary() -> None:
    if not RESULTS_FILE.exists():
        return

    summary_file = Path(__file__).parent / ".summary.md"
    with summary_file.open("w") as f:
        # Read the RESULTS_FILE and generate a markdown summary
        f.write("# Test Summary\n\n")
        f.write("""
This document contains a summary of the test results for the wheels in the `wheels.toml` file.
You can use the following command, in the pixi repository, to filter tests by a specific wheel:
```bash
pixi r test-common-wheels -k "<pixi_add_cmd>"
# E.g
pixi r test-common-wheels-dev -k "jax[cuda12]"
```

""")
        f.write("## Test Results\n\n")
        f.write("\n")
        f.write("| Test Name | Outcome | Duration (s) | Error Details |\n")
        f.write("| :--- | ---: | ---: | --- |\n")

        results_file = RESULTS_FILE
        with results_file.open("rb") as r:
            results = tomllib.load(r)["results"]
            for result in results:
                outcome = (
                    '<span style="color: green">Passed</span>'
                    if result["outcome"] == "passed"
                    else '<span style="color: red">Failed</span>'
                )
                error_details = result["longrepr"] if result["outcome"] == "failed" else ""
                f.write(f"|{result['name']}|{outcome}|{result['duration']:.2f}|{error_details}|\n")
        f.write("\n")
