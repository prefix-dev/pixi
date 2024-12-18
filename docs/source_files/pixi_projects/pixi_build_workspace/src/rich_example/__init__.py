from dataclasses import dataclass, fields
from rich.console import Console
from rich.table import Table
import python_bindings
from datetime import datetime


@dataclass
class Person:
    name: str
    age: int
    city: str


def main() -> None:
    console = Console()

    years_since_2020 = datetime.now().year - 2020

    people = [
        Person("John Doe", 30, "New York"),
        Person("Jane Smith", 25, "Los Angeles"),
        Person("Tim de Jager", 35, "Utrecht"),
    ]

    table = Table()

    for column in fields(Person):
        table.add_column(column.name)

    for person in people:
        updated_age = python_bindings.add(person.age, years_since_2020)
        table.add_row(person.name, str(updated_age), person.city)

    console.print(table)
