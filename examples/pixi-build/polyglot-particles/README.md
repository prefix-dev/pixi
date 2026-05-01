# Polyglot Particles

A pixi-build workspace that wires together six packages written in four
languages around a single C ABI. The point is to show how `pixi-build` lets
you depend on path-based, multi-language packages from one workspace and run
them with one command.

```
particle_core      C       shared lib + headers, defines the ABI
particle_cpp       C++     emitter/modifier impls, exposes vtables
particle_cpp_py    C++     pybind11 module wrapping particle_cpp
particle_rs        Rust    emitter/modifier impls + PyO3 bindings
particle_view      C++     pybind11 module that opens an SDL window
particle_kit       Python  high-level API and entry point
```

Two pixi-build backends do all the work: `pixi-build-cmake` for the C and C++
packages (including the two pybind11 Python extensions, which install
themselves directly into the host environment's `Python_SITEARCH`), and
`pixi-build-python` for the Rust crate (via maturin) and the pure-Python kit
(via hatchling).

## Run it

```sh
pixi run demo   # build a Python scene and visualize it
pixi run list   # print every emitter/modifier registered by the cpp + rust libs
```

## How the pieces fit

`particle_core` ships a header with two vtable types (`pc_emitter_vtable_t`,
`pc_modifier_vtable_t`) plus a runtime `pc_pool_t` that integrates particles,
runs modifiers, and pulls from emitters each step. Both `particle_cpp` and
`particle_rs` implement that interface in their own language and export
discovery functions (`particle_cpp_get_emitter("cone")`, etc.).

`particle_cpp_py` (pybind11) and `particle_rs` (PyO3) each surface their own
emitters/modifiers as Python objects. Each object exposes `vtable_addr` and
`state_addr` as integers. `particle_view` is a pybind11 module whose only
function is `View(w, h).run(emitters=[...], modifiers=[...])` which reads
those addresses, builds a `pc_pool_t` internally, and runs the SDL loop.
No `dlopen` happens between packages; the Python side just hands raw
function-pointer integers across the boundary.

`particle_kit` is the orchestrator: it imports both binding modules and the
view, and ships a `__main__` that builds a small scene as a smoke test.
