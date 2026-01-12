from typing import Optional
from pixi_build_backend.pixi_build_backend import (
    PyPlatform,
)


class Platform:
    """
    Example class for platform handling.
    This should implement the Platform trait.
    """

    _inner = PyPlatform

    def __init__(self, value: str) -> None:
        self._inner = PyPlatform(value)

    @classmethod
    def current(cls) -> "Platform":
        """
        Returns the current platform.
        """
        return cls._from_py(PyPlatform.current())

    @classmethod
    def _from_py(cls, py_platform: PyPlatform) -> "Platform":
        """Construct Rattler version from FFI PyArch object."""
        return cls(py_platform.name)

    def __str__(self) -> str:
        return str(self._inner)

    def __repr__(self) -> str:
        return "Platform({})".format(str(self._inner))

    @property
    def is_linux(self) -> bool:
        """
        Return True if the platform is linux.

        Examples
        --------
        ```python
        >>> Platform("linux-64").is_linux
        True
        >>> Platform("osx-64").is_linux
        False
        >>>
        ```
        """
        return self._inner.is_linux

    @property
    def is_osx(self) -> bool:
        """
        Return True if the platform is osx.

        Examples
        --------
        ```python
        >>> Platform("osx-64").is_osx
        True
        >>> Platform("linux-64").is_osx
        False
        >>>
        ```
        """
        return self._inner.is_osx

    @property
    def is_windows(self) -> bool:
        """
        Return True if the platform is win.

        Examples
        --------
        ```python
        >>> Platform("win-64").is_windows
        True
        >>> Platform("linux-64").is_windows
        False
        >>>
        ```
        """
        return self._inner.is_windows

    @property
    def is_unix(self) -> bool:
        """
        Return True if the platform is unix.

        Examples
        --------
        ```python
        >>> Platform("linux-64").is_unix
        True
        >>> Platform("win-64").is_unix
        False
        >>>
        ```
        """
        return self._inner.is_unix

    @property
    def only_platform(self) -> Optional[str]:
        """
        Return the platform without the architecture.

        Examples
        --------
        ```python
        >>> Platform("linux-64").only_platform
        'linux'
        >>>
        ```
        """
        return self._inner.only_platform
