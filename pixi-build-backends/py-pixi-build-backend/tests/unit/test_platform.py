"""Unit tests for platform.py module."""

from pixi_build_backend.types.platform import Platform


def test_current_class_method() -> None:
    """Test creation of Platform from string and its underlying magic methods."""
    result = Platform("linux-64")

    assert str(result) == "linux-64"
    assert result.is_linux
