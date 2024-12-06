from rich.console import Console
from rich.table import Table


def main() -> None:
    console = Console()

    table = Table(title="Simple Rich Example")

    table.add_column("Name", justify="right", style="cyan", no_wrap=True)
    table.add_column("Age", style="magenta")
    table.add_column("City", justify="right", style="green")

    table.add_row("John Doe", "30", "New York")
    table.add_row("Jane Smith", "25", "Los Angeles")
    table.add_row("Tim de Jager", "35", "Utrecht")

    console.print(table)
