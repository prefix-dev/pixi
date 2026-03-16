from pixi_build_backend.pixi_build_backend import PyPythonParams


class PythonParams:
    """A Python parameters wrapper."""

    _inner: PyPythonParams

    def __init__(self, editable: bool = False):
        """
        Initialize PythonParams.

        Parameters
        ----------
        editable : bool, optional
            Whether to enable editable mode, by default False

        Examples
        --------
        ```python
        >>> params = PythonParams()
        >>> params.editable
        False
        >>> params_editable = PythonParams(editable=True)
        >>> params_editable.editable
        True
        >>>
        ```
        """
        self._inner = PyPythonParams(editable=editable)

    @property
    def editable(self) -> bool:
        """
        Get the editable flag.

        Examples
        --------
        ```python
        >>> params = PythonParams(editable=True)
        >>> params.editable
        True
        >>>
        ```
        """
        return self._inner.editable

    @editable.setter
    def editable(self, value: bool) -> None:
        """
        Set the editable flag.

        Examples
        --------
        ```python
        >>> params = PythonParams()
        >>> params.editable = True
        >>> params.editable
        True
        >>>
        ```
        """
        self._inner.set_editable(value)

    def __repr__(self) -> str:
        return self._inner.__repr__()

    @classmethod
    def _from_py(cls, inner: PyPythonParams) -> "PythonParams":
        """Create a PythonParams from a FFI PyPythonParams."""
        instance = cls.__new__(cls)
        instance._inner = inner
        return instance
