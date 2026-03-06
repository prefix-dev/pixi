from typing import Optional
from pixi_build_backend.pixi_build_backend import PyPackageDependency, PySourceMatchSpec


class SourceMatchSpec:
    """A source match spec wrapper."""

    _inner: PySourceMatchSpec

    def __init__(self, spec: str, location: str) -> None:
        self._inner = PySourceMatchSpec(spec, location)

    @property
    def spec(self) -> str:
        """Get the spec."""
        return self._inner.spec

    @property
    def location(self) -> str:
        """Get the location."""
        return self._inner.location

    def _into_py(self) -> PySourceMatchSpec:
        """Convert to PySourceMatchSpec."""
        return self._inner

    @classmethod
    def _from_inner(cls, inner: PySourceMatchSpec) -> "SourceMatchSpec":
        """Create from PySourceMatchSpec."""
        instance = cls.__new__(cls)
        instance._inner = inner
        return instance


class PackageDependency:
    """A package dependency wrapper."""

    _inner: PyPackageDependency

    def __init__(self, spec: str) -> None:
        self._inner = PyPackageDependency(spec)

    @property
    def is_binary(self) -> bool:
        """Check if this is a binary dependency."""
        return self._inner.is_binary()

    @property
    def is_source(self) -> bool:
        """Check if this is a source dependency."""
        return self._inner.is_source()

    @property
    def binary_spec(self) -> Optional[str]:
        """Get the binary spec if this is a binary dependency."""
        return self._inner.get_binary()

    @property
    def source_spec(self) -> Optional[SourceMatchSpec]:
        """Get the source spec if this is a source dependency."""
        inner_spec = self._inner.get_source()
        return SourceMatchSpec._from_inner(inner_spec) if inner_spec else None

    @property
    def package_name(self) -> str:
        """Get the package name."""
        return self._inner.package_name()

    def _into_py(self) -> PyPackageDependency:
        """Convert to PyPackageDependency."""
        return self._inner

    @classmethod
    def _from_inner(cls, inner: PyPackageDependency) -> "PackageDependency":
        """Create from PyPackageDependency."""
        instance = cls.__new__(cls)
        instance._inner = inner
        return instance

    def __str__(self) -> str:
        """Return string representation."""
        return str(self._inner)
