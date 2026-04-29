from typing import Protocol, runtime_checkable

import particle_cpp_py as cpp
import particle_rs as rs
from particle_view import View

from . import registry


@runtime_checkable
class Emitter(Protocol):
    def c_interface(self) -> tuple[int, int, int]:
        """Returns (data_addr, emit_fn_addr, destroy_fn_addr)."""
        ...


@runtime_checkable
class Modifier(Protocol):
    def c_interface(self) -> tuple[int, int, int]:
        """Returns (data_addr, apply_fn_addr, destroy_fn_addr)."""
        ...


__all__ = ["cpp", "rs", "View", "Emitter", "Modifier", "registry"]
