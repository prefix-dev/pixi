"""
A panic-inducing Python package for testing purposes.
"""

__version__ = "0.1.0"


def panic():
    """Raise a panic exception."""
    raise Exception("PANIC! This is an intentional exception from panic-panic package!")


def controlled_panic(message="Controlled panic occurred"):
    """Raise a controlled panic with custom message."""
    raise RuntimeError(f"PANIC: {message}")
