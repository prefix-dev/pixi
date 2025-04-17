from dataclasses import dataclass, fields
from rich.console import Console
from rich.table import Table
import cpp_math


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
        updated_age = cpp_math.add(person.age, 1)
        table.add_row(person.name, str(updated_age), person.city)

    console.print(table)
