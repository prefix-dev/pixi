from dataclasses import dataclass, fields
from rich.console import Console
from rich.table import Table


@dataclass
class Person:
    name: str
    age: int
    city: str


def main() -> None:
    console = Console()

    people = [
        Person("John Doe", 30, "New York"),
        Person("Jane Smith", 25, "Los Angeles"),
        Person("Tim de Jager", 35, "Utrecht"),
    ]

    table = Table()

    for column in fields(Person):
        table.add_column(column.name)

    for person in people:
        table.add_row(person.name, str(person.age), person.city)

    console.print(table)
