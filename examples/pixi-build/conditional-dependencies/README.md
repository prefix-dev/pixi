# Conditional dependencies

A minimal C++ package that shows off `if(...)` conditional dependencies in a
Pixi `[package]` section.

`cuda_probe` is a tiny executable that reports which compiler built it and
whether it was compiled with CUDA support. The CUDA runtime is only available
on conda-forge for Linux and Windows, so the manifest asks for it only there:

```toml
[package.host-dependencies."if(linux or win)"]
cuda-version = "12.*"
cuda-cudart-dev = "12.*"
```

On Linux/Windows the build links against the CUDA runtime and the program
queries the visible device count. On macOS the block is skipped entirely and
the very same sources build a CPU-only binary.

## Run it

```bash
pixi run start
```

On a machine without conditional-dependency support you would instead see the
old `[package.target.*]` tables. The `if(<expression>)` form accepts anything
rattler-build understands (`and`, `or`, `not`, `matches(...)`, ...) and exposes
these variables:

- `build_platform`  the platform the build runs on
- `host_platform`   the platform the package is built for (differs when cross-compiling)
- `target_platform` the run platform (differs from `host_platform` for `noarch`)
- the bare booleans `unix`, `linux`, `win` and `osx`
