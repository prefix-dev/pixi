In this tutorial, we will show you how to build a **nanobind** Python binding that supports **cross-compilation** (e.g. `linux-64` → `linux-aarch64`).
In this tutorial we assume that you've read the [Building a C++ Package](cpp.md) tutorial.
If you haven't read it yet, we recommend you to do so before continuing.
You might also want to check out the [documentation](backends/pixi-build-rattler-build.md) for the `pixi-build-rattler-build` backend.
The project structure and the source code will be the same as in the previous tutorial, so we may skip explicit explanations of some parts.

We use [`rattler-build`](https://rattler.build) as the build backend, via the `pixi-build-rattler-build` backend, and split the output into **three packages**:

| Package | Type | Built on | Installed on |
|---|---|---|---|
| `cpp_math` | native `.so` | every target platform | matching platform |
| `cpp_math-stubs` | `noarch: python` | `linux-64` only | all platforms |

> **Why the split?**
> Generating stubs (`.pyi` files) requires *importing* the compiled `.so`, and calling Python Executable on the targeted platform which is impossible when cross-compiling.
> By making the stubs a separate `noarch` package built only on the host, they remain shareable across all target platforms.

!!! warning
    `pixi-build` is a preview feature and will change until it is stabilized.


## Workspace structure

To get started, create a new workspace with pixi:

```bash
pixi init cpp_math
```

This should give you the basic `pixi.toml` to get started.

We'll now create the following source directory structure:
```bash
.
├── CMakeLists.txt
├── pixi.toml
├── recipe/
│   └── recipe.yml
└── src/
    └── math.cpp
```

## The source file

`src/math.cpp` exposes a single `add` function using nanobind:

```cpp
#include <nanobind/nanobind.h>

int add(int a, int b) { return a + b; }

NB_MODULE(cpp_math, m)
{
    m.def("add", &add);
}
```

## The `CMakeLists.txt`

The CMake file handles three scenarios:

1. **Cross-compiling** (`CMAKE_CROSSCOMPILING=ON`): Python is not executable on the host, so we locate nanobind directly from the sysroot (`$PREFIX`).
2. **Native build with `$PREFIX`**: normal case during packaging.
3. **Stubs-only build** (`STUBS_ONLY=ON`): the `.so` is already installed; we only call `nanobind_add_stub`.

```cmake
cmake_minimum_required(VERSION 3.15)
cmake_policy(SET CMP0190 NEW)

project(cpp_math)

option(STUBS_ONLY "Only generate stubs (module already installed)" OFF)

# ── Cross-compilation ─────────────────────────────────────────────────────────
if(CMAKE_CROSSCOMPILING AND DEFINED ENV{PREFIX})
  message(STATUS "Cross-compiling, detecting Python from sysroot…")

  set(nanobind_ROOT        "$ENV{PREFIX}/lib/python$ENV{PY_VER}/site-packages/nanobind/cmake")
  set(PYTHON_SITE_PACKAGES "$ENV{PREFIX}/lib/python$ENV{PY_VER}/site-packages")

  find_package(Python $ENV{PY_VER} EXACT COMPONENTS Development.Module REQUIRED)

elseif(CMAKE_CROSSCOMPILING)
  message(FATAL_ERROR "Cross-compiling but PREFIX is not set.")

elseif(DEFINED ENV{PREFIX})
  find_package(Python $ENV{PY_VER} EXACT COMPONENTS Interpreter Development.Module REQUIRED)
  execute_process(
    COMMAND "${Python_EXECUTABLE}" -m nanobind --cmake_dir
    OUTPUT_STRIP_TRAILING_WHITESPACE OUTPUT_VARIABLE nanobind_ROOT
  )
  execute_process(
    COMMAND "${Python_EXECUTABLE}" -c
      "import sysconfig; print(sysconfig.get_path('purelib'))"
    OUTPUT_VARIABLE PYTHON_SITE_PACKAGES
    OUTPUT_STRIP_TRAILING_WHITESPACE
  )
endif()
# ─────────────────────────────────────────────────────────────────────────────

find_package(nanobind CONFIG REQUIRED)

# ── Compiled extension ────────────────────────────────────────────────────────
if(NOT STUBS_ONLY)
  nanobind_add_module(cpp_math src/math.cpp)

  install(
    TARGETS cpp_math
    LIBRARY DESTINATION ${PYTHON_SITE_PACKAGES}/cpp_math
    ARCHIVE DESTINATION ${PYTHON_SITE_PACKAGES}/cpp_math
  )
endif()

# ── Stubs ─────────────────────────────────────────────────────────────────────
if(STUBS_ONLY)
  # The .so is already installed as a dependency of this package.
  nanobind_add_stub(
    cpp_math_stub
    MODULE    cpp_math
    RECURSIVE
    OUTPUT    cpp_math.pyi
    MARKER_FILE py.typed
    OUTPUT_PATH ${PYTHON_SITE_PACKAGES}/cpp_math
    PYTHON_PATH ${PYTHON_SITE_PACKAGES}/cpp_math
  )
endif()
```

**Key points:**

- When cross-compiling, `find_package(Python … Development.Module)` finds the *target* headers in `$PREFIX` without needing a runnable interpreter.
- `nanobind_ROOT` is set manually to the nanobind CMake helpers bundled in the target `$PREFIX` — because `python -m nanobind --cmake_dir` cannot run cross-compiled code.
- `STUBS_ONLY` lets the same CMake project build just the `.pyi` files in a second, native-only pass.

---

## The `pixi.toml`

```toml
[workspace]
channels  = ["https://prefix.dev/conda-forge"]
platforms = ["linux-64", "linux-aarch64"]
preview   = ["pixi-build"]

[dependencies]
cpp_math = { path = "." }
python   = "*"

[package]
name    = "cpp_math"
version = "0.1.0"

[package.build]
backend = { name = "pixi-build-rattler-build", version = "*" }

[tasks]
start = "python -c 'import cpp_math; print(cpp_math.add(1, 2))'"
```

The workspace lists **both** target platforms. pixi will automatically cross-compile the `linux-aarch64` variant when calling `pixi build --target-platform linux-aarch64`.

---

## The `recipe/recipe.yml`

This is the heart of the build. The recipe declares **two outputs** from the same source tree.

```yaml
context:
  version: 0.1.0

source:
  path: ../   # (1)

outputs:

  # ── 1. Compiled extension — built for every target platform ─────────────────
  - package:
      name: cpp_math
      version: ${{ version }}
    build:
      number: 0
      script:
        - if: unix
          then: |
            mkdir -p build && rm -rf build/*
            cmake -GNinja -Bbuild -S .    \
              ${CMAKE_ARGS}               \
              -DSTUBS_ONLY=OFF            \
              -DCMAKE_INSTALL_PREFIX=$PREFIX \
              -DCMAKE_BUILD_TYPE=Release
            ninja -C build
            ninja -C build install
    requirements:
      build:    # (2)
        - ${{ compiler('cxx') }}
        - cmake
        - ninja
      host:     # (3)
        - python
        - nanobind >=2.0.0
      run:
        - python

  # ── 2. Stubs — noarch, built only on the host (linux-64) ───────────────────
  - package:
      name: cpp_math-stubs
      version: ${{ version }}
    build:
      number: 0
      noarch: python    # (4)
      skip:
        - build_platform == "linux-aarch64"   # (5)
      script:
        - if: unix
          then: |
            mkdir -p build && rm -rf build/*
            cmake -GNinja -Bbuild -S .    \
              ${CMAKE_ARGS}               \
              -DSTUBS_ONLY=ON             \
              -DCMAKE_INSTALL_PREFIX=$PREFIX \
              -DCMAKE_BUILD_TYPE=Release
            ninja -C build cpp_math_stub
    requirements:
      build:
        - cmake
        - ninja
      host:
        - python
        - nanobind >=2.0.0
        - cpp_math    # (6)
      run:
        - python
```

1. **`source.path: ../`** — points to the workspace root. rattler-build may skip untracked files; make sure your source files are tracked by git, or use `git_url` instead.
2. **`build` dependencies** run on the *host machine* (the compiler). `${{ compiler('cxx') }}` resolves to the right cross-compiler automatically.
3. **`host` dependencies** are installed in the *target sysroot* (`$PREFIX`). Python and nanobind headers are there, not in the build environment.
4. **`noarch: python`** means the stubs package contains only Python files (`.pyi`, `py.typed`) and can be installed on any platform without recompilation.
5. **`skip` on `linux-aarch64`** prevents rattler-build from trying to run stubs on a cross-compiled build where the `.so` cannot be imported natively.
6. **`cpp_math` in `host`** installs the native `.so` into the stub-generation environment so `nanobind_add_stub` can import it to introspect the module.

!!! note "CMake Arguments"
    There are two CMake variables that you should pass in your recipe:

    1. `${CMAKE_ARGS}` which forwards the `CMAKE_CROSSCOMPILING` variable to the CMakeList.
	2. `STUBS_ONLY=ON` which allows to specify if the stubs should be generated.
---

## Testing

### Native build

```bash
pixi build --output-dir output
```

This produces two packages under `output/`:

```
output/
└── cpp_math-0.1.0-Linux64Hash_0.conda          ← compiled extension for linux-64
└── cpp_math-stubs-0.1.0-Linux64Hash_0.conda    ← stubs (platform-independent)
```

### Cross-compilation build

```bash
pixi build --target-platform linux-aarch64
```

A third package is added:

```
output/
└── cpp_math-0.1.0-Linux64Hash_0.conda          ← compiled extension for linux-64
└── cpp_math-stubs-0.1.0-Linux64Hash_0.conda    ← stubs (platform-independent)
└── cpp_math-0.1.0-LinuxAarch64Hash_0.conda     ← compiled extension for linux-64
```

The stubs package is **not** rebuilt: since it is `noarch`, the one produced during the native build can be reused on `linux-aarch64` as well.

### Verifying the ELF architecture

To check that the packages have the correct architecture, run :

```bash
unzip -q output/YOUR_PACKAGE.conda -d tmp && \
tar --use-compress-program=unzstd -xf tmp/info-*.tar.zst -C tmp && \
jq '.platform, .subdir' tmp/info/index.json && \
rm -rf tmp
```

For the `linux-64` package, output should be
```bash
"linux"
"linux-64"
```

For the `linux-aarch64` package, output should be
```bash
"linux"
"linux-aarch64"
```

For the `stub` package, output should be
```bash
"null"
"noarch"
```

---

## Summary

| Concern | Solution |
|---|---|
| Cross-compilation breaks `find_package(Python)` | Locate nanobind/Python manually via `$PREFIX` |
| Stubs require running the `.so` | Separate `noarch` package, skipped on cross builds |
| Same CMakeLists for both passes | `STUBS_ONLY` option switches behaviour |
| Stubs still available on `aarch64` | `noarch: python` package is platform-independent |
