from typing import Optional
from typing_extensions import TypeAlias
from pixi_build_backend.pixi_build_backend import (
    PyConditionalString,
    PyListOrItemString,
    PyListOrItemPackageDependency,
    PyConditionalPackageDependency,
)


# Type aliases for FFI types
ListOrItemString: TypeAlias = PyListOrItemString
ListOrItemPackageDependency: TypeAlias = PyListOrItemPackageDependency


class ConditionalString:
    _inner: PyConditionalString

    def __init__(self, condition: str, then: ListOrItemString, else_: Optional[ListOrItemString]) -> None:
        else_ = else_ if else_ is not None else ListOrItemString([])
        self._inner = PyConditionalString(condition, then, else_)

    @property
    def condition(self) -> str:
        """Get the condition string."""
        return self._inner.condition

    @property
    def then_value(self) -> "ListOrItemString":
        """Get the then value."""
        return self._inner.then_value
        # return result

    @property
    def else_value(self) -> ListOrItemString:
        """Get the else value."""
        return self._inner.else_value

    def __str__(self) -> str:
        return str(self._inner)

    def __eq__(self, other: object) -> bool:
        """Check equality."""
        if not isinstance(other, ConditionalString):
            return False
        return self._inner == other._inner


class ConditionalPackageDependency:
    _inner: PyConditionalPackageDependency

    def __init__(self, condition: str, then: ListOrItemPackageDependency, else_: ListOrItemPackageDependency) -> None:
        self._inner = PyConditionalPackageDependency(condition, then, else_)

    @property
    def condition(self) -> str:
        """Get the condition string."""
        return self._inner.condition

    @condition.setter
    def condition(self, value: str) -> None:
        """Set the condition string."""
        self._inner.condition = value

    @property
    def then_value(self) -> ListOrItemPackageDependency:
        """Get the then value."""
        return self._inner.then_value

    @property
    def else_value(self) -> ListOrItemPackageDependency:
        """Get the else value."""
        return self._inner.else_value
