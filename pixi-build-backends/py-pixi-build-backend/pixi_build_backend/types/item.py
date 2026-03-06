from __future__ import annotations
from typing import List, Optional, Iterable, Any, Union, overload, MutableSequence
from typing_extensions import SupportsIndex
from pixi_build_backend.pixi_build_backend import PyVecItemPackageDependency, PyItemPackageDependency
from pixi_build_backend.types.conditional import ConditionalPackageDependency
from pixi_build_backend.types.requirements import PackageDependency


class VecItemPackageDependency(MutableSequence["ItemPackageDependency"]):
    """A wrapper for a list of ItemPackageDependency."""

    _inner: PyVecItemPackageDependency

    def __init__(self, items: Optional[List[ItemPackageDependency]] = None) -> None:
        """Initialize with optional items."""
        if items is None:
            self._inner = PyVecItemPackageDependency()
        else:
            # Extract _inner from ItemPackageDependency objects
            inner_items = [item._inner for item in items]
            self._inner = PyVecItemPackageDependency(inner_items)

    @classmethod
    def _from_inner(cls, inner: PyVecItemPackageDependency) -> VecItemPackageDependency:
        """Create from PyVecItemPackageDependency."""
        instance = cls.__new__(cls)
        instance._inner = inner
        return instance

    def __len__(self) -> int:
        """Return the length of the list."""
        return len(self._inner)

    @overload
    def __getitem__(self, index: int) -> ItemPackageDependency: ...

    @overload
    def __getitem__(self, index: slice) -> VecItemPackageDependency: ...

    def __getitem__(self, index: Union[int, slice]) -> Union[ItemPackageDependency, VecItemPackageDependency]:
        """Get item at index."""
        if isinstance(index, slice):
            return VecItemPackageDependency._from_inner(self._inner[index])
        return ItemPackageDependency._from_inner(self._inner[index])

    @overload
    def __setitem__(self, index: int, value: ItemPackageDependency) -> None: ...

    @overload
    def __setitem__(self, index: slice, value: Iterable[ItemPackageDependency]) -> None: ...

    def __setitem__(
        self, index: Union[int, slice], value: Union[ItemPackageDependency, Iterable[ItemPackageDependency]]
    ) -> None:
        """Set item at index."""
        if isinstance(index, slice):
            if isinstance(value, Iterable):
                inner_values = [item._inner for item in value]
                self._inner[index] = inner_values
        else:
            if isinstance(value, ItemPackageDependency):
                inner_value = value._inner
                self._inner[index] = inner_value

    def __delitem__(self, index: Union[int, slice]) -> None:
        """Delete item at index."""
        del self._inner[index]

    def __iter__(self) -> Any:
        """Return iterator."""
        for item in self._inner.__iter__():
            yield ItemPackageDependency._from_inner(item)

    def __contains__(self, item: object) -> bool:
        """Check if item is in the list."""
        if isinstance(item, ItemPackageDependency):
            return item._inner in self._inner
        return item in self._inner

    def append(self, item: ItemPackageDependency) -> None:
        """Append item to the list."""
        inner_item = item._inner
        self._inner.append(inner_item)

    def extend(self, items: Iterable[ItemPackageDependency]) -> None:
        """Extend the list with items."""
        inner_items = [item._inner for item in items]
        self._inner.extend(inner_items)

    def insert(self, index: SupportsIndex, item: ItemPackageDependency) -> None:
        """Insert item at index."""
        inner_item = item._inner
        self._inner.insert(index, inner_item)

    def remove(self, item: ItemPackageDependency) -> None:
        """Remove first occurrence of item."""
        inner_item = item._inner
        self._inner.remove(inner_item)

    def pop(self, index: SupportsIndex = -1) -> ItemPackageDependency:
        """Remove and return item at index."""
        return ItemPackageDependency._from_inner(self._inner.pop(index))

    def clear(self) -> None:
        """Remove all items."""
        self._inner.clear()

    def index(self, item: ItemPackageDependency, start: SupportsIndex = 0, stop: Optional[SupportsIndex] = None) -> int:
        """Return index of first occurrence of item."""
        inner_item = item._inner
        if stop is None:
            return self._inner.index(inner_item, start)
        else:
            return self._inner.index(inner_item, start, stop)

    def count(self, item: ItemPackageDependency) -> int:
        """Return count of occurrences of item."""
        inner_item = item._inner
        return self._inner.count(inner_item)

    def reverse(self) -> None:
        """Reverse the list in place."""
        self._inner.reverse()

    def sort(self, key: Optional[Any] = None, reverse: bool = False) -> None:
        """Sort the list in place."""
        if key is None:
            self._inner.sort(reverse=reverse)
        else:
            self._inner.sort(key=key, reverse=reverse)

    def copy(self) -> VecItemPackageDependency:
        """Return a shallow copy."""
        return VecItemPackageDependency._from_inner(self._inner.copy())

    def __eq__(self, other: Any) -> bool:
        """Check equality."""
        if isinstance(other, VecItemPackageDependency):
            return self._inner == other._inner
        return False

    def __str__(self) -> str:
        """Return string representation."""
        return str(self._inner)


class ItemPackageDependency:
    """A package dependency item wrapper."""

    _inner: PyItemPackageDependency

    def __init__(self, name: str):
        self._inner = PyItemPackageDependency(name)

    @classmethod
    def new_from_conditional(cls, conditional: ConditionalPackageDependency) -> ItemPackageDependency:
        new_class = cls.__new__(cls)
        new_class._inner = PyItemPackageDependency.new_from_conditional(conditional._inner)
        return new_class

    @classmethod
    def _from_inner(cls, inner: PyItemPackageDependency) -> ItemPackageDependency:
        """Create an ItemPackageDependency from a FFI PyItemPackageDependency."""
        instance = cls.__new__(cls)
        instance._inner = inner
        return instance

    def __str__(self) -> str:
        return str(self._inner)

    @property
    def concrete(self) -> Optional["PackageDependency"]:
        """Get the concrete package dependency."""
        concrete = self._inner.concrete()
        if concrete is None:
            return None
        return PackageDependency._from_inner(concrete)

    @property
    def template(self) -> Optional[str]:
        """Get the template string if this is a template."""
        return self._inner.template()

    @property
    def conditional(self) -> Optional[ConditionalPackageDependency]:
        """Get the conditional string if this is a conditional."""
        return self._inner.conditional()
