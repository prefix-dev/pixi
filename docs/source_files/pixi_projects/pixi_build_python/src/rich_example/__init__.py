from rich.console import Console
from rich.table import Table


def main() -> None:
    console = Console()

    table = Table()

    table.add_column("Name")
    table.add_column("Age")
    table.add_column("City")

    table.add_row("John Doe", "30", "New York")
    table.add_row("Jane Smith", "25", "Los Angeles")
    table.add_row("Tim de Jager", "35", "Utrecht")

    console.print(table)
