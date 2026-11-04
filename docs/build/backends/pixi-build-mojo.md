# pixi-build-mojo

The `pixi-build-mojo` backend is designed for building Mojo projects. It provides seamless integration with Pixi's package management workflow.

!!! warning
    `pixi-build` is a preview flag, and will change until it is stabilized.
    This is why we require users to opt in to that feature by adding "pixi-build" to `workspace.preview`.

    ```toml
    [workspace]
    preview = ["pixi-build"]
    ```

## Overview

This backend automatically generates conda packages from Mojo projects.

The generated packages can be installed into local envs for development, or packaged for distribution.

On macOS, executable packages include an exact runtime dependency on `mojo-compiler`, which also supplies `libKGENCompilerRTShared.dylib`.

!!! warning "Cross-compilation"
    Cross-compiling between conda platforms is not supported by `pixi-build-mojo`.
    The `mojo-compiler` activation script runs `python3` from the environment being activated, which can fail when it targets a different operating system or architecture.
    Build on a machine matching the package's target platform.

### Auto-derive of pkg and bin

The Mojo backend includes auto-discovery of your project structure and will derive the following:

- **Binaries**: Automatically searches for `main.mojo` in:
  - `<project_root>/main.mojo`
- **Packages**: Automatically searches for directories with `__init__.mojo` in:
  - `<project_root>/<project_name>/`
  - `<project_root>/src/`

This means in most cases, you don't need to explicitly configure the `bins` or `pkg` fields.

**Caveats**:

- If both a `bin` and a `pkg` are auto-derived, only the `bin` will be created, you must manually specify the pkg.
- If the user specifies a `pkg` a `bin` will not be auto-derived.
- If the user specifies a `bin` a `pkg` will not be auto-derived.


## Basic Usage

To use the Mojo backend in your `pixi.toml`, add it to your package's build configuration. The backend will automatically discover your project structure:


```txt
# Example project layout for combined binary/library.
.
├── greetings
│   ├── __init__.mojo
│   └── lib.mojo
├── main.mojo
├── pixi.lock
├── pixi.toml
└── README.md
```

With the project structure above, pixi-build-mojo will automatically discover:

- The binary from `main.mojo`
- The package from `greetings/__init__.mojo`

Here's a minimal configuration that leverages auto-derive:

```toml
[workspace]
authors = ["J. Doe <jdoe@mail.com>"]
platforms = ["linux-64"]
preview = ["pixi-build"]
channels = [
    "https://prefix.dev/conda-forge",
    "https://conda.modular.com/max-nightly",
    "https://prefix.dev/modular-community"
]

[package]
name = "greetings"
version = "0.1.0"

[package.build]
backend = { name = "pixi-build-mojo", version = "0.*" }

[tasks]

[package.host-dependencies]
mojo-compiler = "=25.5.0"

[package.build-dependencies]
mojo-compiler = "=25.5.0"
small_time = ">=25.4.1,<26"
extramojo = ">=0.16.0,<0.17"

[package.run-dependencies]
mojo-compiler = "=25.5.0"

[dependencies]
# For running `mojo test` while developing add all dependencies under
# `[package.build-dependencies]` here as well.
greetings = { path = "." }
```

### Project Structure Examples

The auto-derive feature supports various common project layouts:

#### Binary-only project
```txt
.
├── main.mojo           # Auto-derive as binary
├── pixi.toml
└── README.md
```

#### Package-only project
```txt
.
├── mypackage/          # Auto-derive if matches project name
│   ├── __init__.mojo
│   └── utils.mojo
├── pixi.toml
└── README.md
```

#### Source directory layout
```txt
.
├── src/
│   ├── __init__.mojo   # Auto-derive as package
│   └── lib.mojo
├── pixi.toml
└── README.md
```

#### Combined project (shown earlier)
```txt
.
├── greetings/
│   ├── __init__.mojo   # NOT auto-derived as package
│   └── lib.mojo
├── main.mojo           # Auto-derived as binary
├── pixi.toml
└── README.md
```

### Required Dependencies

- `mojo` / `mojo-compiler` package for both the compiler and linked runtime

## Configuration Options

You can customize the Mojo backend behavior using the `[package.build.config]` section in your `pixi.toml`. The backend supports the following configuration options:

#### `env`

- **Type**: `Map<String, String>`
- **Default**: `{}`

Environment variables to set during the build process.

```toml
[package.build.config]
env = { ASSERT = "all" }
```

#### `debug-dir`

The backend always writes JSON-RPC request/response logs and the generated intermediate recipe to the `debug` subdirectory inside the work directory (for example `<work_directory>/debug`). The deprecated `debug-dir` configuration option is ignored. If present, the backend emits a warning to highlight that it no longer has any effect.

#### `extra-input-globs`

- **Type**: `Array<String>`
- **Default**: `[]`

Additional globs to pass to pixi to discover if the package should be rebuilt.

```toml
[package.build.config]
extra-input-globs = ["**/*.c", "assets/**/*", "*.md"]
```

### `compilers`

- **Type**: `Array<String>`
- **Default**: `[]`
- **Target Merge Behavior**: `Overwrite` - Platform-specific compilers completely replace base compilers

List of compilers to use for the build. Compilers use conda-forge's standard compiler infrastructure.

```toml
[package.build.config]
compilers = ["c", "cxx"]
```

For target-specific configuration, platform compilers completely replace the base configuration:

```toml
[package.build.config]
compilers = []

[package.build.target.linux-64.config]
compilers = ["c", "cuda"]
# Result for linux-64: ["mojo", "c", "cuda"]
```

!!! info "Comprehensive Compiler Documentation"
    For detailed information about available compilers, platform-specific behavior, and how conda-forge compilers work, see the [Compilers Documentation](../key_concepts/compilers.md). Note that the mojo compiler has special behavior as described above.

### `bins`

- **Type**: `Array<BinConfig>`
- **Default**: Auto-derived if not specified

List of binary configurations to build. The created binary will be placed in the `$PREFIX/bin` dir and will be in the path after running `pixi install` assuming the package is listed as a dependency as in the example above. Running `pixi publish` will create a conda package that includes the binary.

**Auto-derive behavior:**

- If `bins` is not specified, pixi-build-mojo will search for a `main.mojo` file in the project root
- If found, it creates a binary with the name set to the project name
- If a pkg has been manually configured, a bin will not be auto-derived and must be manually configured.

#### `bins[].name`

- **Type**: `String`
- **Default**: Project name (with dashes converted to underscores) for the first binary

The name of the binary executable to create. If not specified:

- For the first binary in the list, defaults to the project name
- For additional binaries, this field is required

```toml
[[package.build.config.bins]]
# name = "greet"  # Optional for first binary, defaults to project name
```

#### `bins[].path`

- **Type**: `String` (path)
- **Default**: Auto-derived for the first binary

The path to the Mojo file that contains a `main` function. If not specified:

- For the first binary, searches for `main.mojo` in the project root
- For additional binaries, this field is required

```toml
[[package.build.config.bins]]
# path = "./main.mojo"  # Optional if main.mojo exists in project root
```

#### `bins[].extra-args`

- **Type**: `Array<String>`
- **Default**: `[]`

Additional command-line arguments to pass to the Mojo compiler when building this binary.

```toml
[[package.build.config.bins]]
extra-args = ["-I", "special-thing"]
```

### `pkg`

- **Type**: `PkgConfig`
- **Default**: Auto-derived if not specified

Package configuration for creating Mojo package. The created Mojo package will be placed in the `$PREFIX/lib/mojo` dir, which will make it discoverable to anything that depends on the package.

**Auto-derive behavior:**

- If `pkg` is not specified, pixi-build-mojo will search for a directory containing `__init__.mojo` in the following order:
  1. `<project_root>/<project_name>/`
  2. `<project_root>/src/`
- If found, it creates a package with the name set to the project name
- If no valid package directory is found, no package is built
- If a binary is manually configured, a pkg will not be auto-derived and must be manually specified.
- If a binary is also auto-derive, a pkg will not be generated and must be manually specified

#### `pkg.name`

- **Type**: `String`
- **Default**: Project name (with dashes converted to underscores)

The name to give the Mojo package. The backend adds the `.mojoc` suffix when using
`mojo precompile`, or `.mojopkg` when falling back to the legacy `mojo package`
command. If not specified, the name defaults to the project name.

```toml
[package.build.config.pkg]
name = "greetings"
```

#### `pkg.path`

- **Type**: `String` (path)
- **Default**: Auto-derive

The path to the directory that constitutes the package. If not specified, searches for a directory with `__init__.mojo` as described above.

```toml
[package.build.config.pkg]
path = "greetings"
```

#### `pkg.extra-args`

- **Type**: `Array<String>`
- **Default**: `[]`

Additional command-line arguments to pass to the Mojo compiler when building this package.

```toml
[package.build.config.pkg]
extra-args = ["-I", "special-thing"]
```

## Default Variants

On Windows platforms, the backend automatically sets the following default variants:

- `c_compiler`: `vs2022` - Visual Studio 2022 C compiler
- `cxx_compiler`: `vs2022` - Visual Studio 2022 C++ compiler

These variants are used when you specify compilers in your [`[package.build.config.compilers]`](#compilers) configuration.
Note that setting these default variants does not automatically add compilers to your build - you still need to explicitly configure which compilers to use.

This default is set to align with conda-forge's switch to Visual Studio 2022 and because [mainstream support for Visual Studio 2019 ended in 2024](https://learn.microsoft.com/en-us/lifecycle/products/visual-studio-2019).
The `vs2022` compiler is more widely supported on modern GitHub runners and build environments.

You can override these defaults by explicitly setting variants using [`[workspace.build-variants]`](https://pixi.sh/latest/reference/pixi_manifest/#build-variants-optional) in your `pixi.toml`:

```toml
[workspace.build-variants]
c_compiler = ["vs2019"]
cxx_compiler = ["vs2019"]
```

## Accelerator variants

Opt in to managed CPU and AMD builds with the canonical `mojo_accelerator` variant:

```toml
[workspace]
platforms = [{ platform = "linux-64", glibc = "2.34", archspec = "x86_64_v3" }]
preview = ["pixi-build"]

[workspace.build-variants]
mojo_accelerator = ["cpu", "gfx1103"]
```

Only `cpu` and `gfx1103` are supported, with exactly this spelling.
Without this variant, the backend only produces binaries that only rely on the CPU.
Managed variants support `linux-64` executable `bins` only, not `pkg` builds, other GPU architectures, or CUDA.
Both outputs keep the same package name, version, executable name, and build number. The variant changes the build hash.

### Compilation

The backend invokes `mojo` through the activated build environment's `PATH` and sets `MODULAR_HOME` to the host environment's `share/max`.
It passes `-D mojo_accelerator=cpu` or `-D mojo_accelerator=gfx1103` so the source can select its implementation at compile time.
The CPU build hides devices with `ROCR_VISIBLE_DEVICES=-1`, `HIP_VISIBLE_DEVICES=-1`, and `CUDA_VISIBLE_DEVICES=-1`.
It does not pass `--target-accelerator none`. CPU code must exclude GPU APIs through a compile-time branch.
The AMD build passes `--target-accelerator gfx1103`.

Both builds use `--target-cpu x86-64-v3 --target-features ''`, matching the prebuilt [Modular runtime's CPU minimum](https://mojolang.org/docs/requirements/#hardware), without host-native specialization.
Both require `_x86_64-microarch-level >=3`, conda-forge's [runtime CPU-capability package](https://prefix.dev/blog/building_cpu_optimized_packages).
Binary `extra-args` are passed through without validation, including options such as `--fp-mode contract=off`.
Callers are responsible for keeping these arguments consistent with the managed CPU/GPU target, `mojo_accelerator` define, executable output, and package compatibility requirements.

The build requirements include `mojo-compiler >=1.2`, conda-forge's `compiler('c')` and `compiler('cxx')` requirements, and `patchelf`.
Compiler activation supplies the prefixed `$CC`. The backend resolves it and sets `MODULAR_MOJO_MAX_LINKER_DRIVER` to that executable.
The C standard library requirement is inferred from the target's declared glibc version through `stdlib('c')`.
Declare `glibc = "2.34"` as above. No compiler metapackages, explicit sysroot pins, or machine-specific compiler paths are needed.

### Runtime and selection

Both outputs add the selected build compiler to the host requirements with `${{ pin_compatible('mojo-compiler', exact=True) }}` and require `max-core >=26.7` there.
The `mojo-compiler` package is needed in both environments because it bundles the compiler executable and the linked `libKGENCompilerRTShared.so` runtime.
Pin `mojo-compiler` in `[package.build-dependencies]` to select its version. The host and runtime requirements follow that selection.
Their runtime requirements exactly pin the selected `mojo-compiler` and `max-core` providers, require `__glibc >=2.34`, and inherit the C and C++ runtimes through compiler run exports.
The AMD output also requires `hip-runtime-amd >=6.3.3`, `__amdgpu`, and `__amdgpu_arch ==11.0.3`.
The CPU output does not require an AMD runtime.

`gfx1103` is an exact ISA requirement, not a minimum GPU generation.
The CPU output has the `track_features` entry `<package-name>-p-0`, which makes it a lower-priority fallback when both builds satisfy the solve.
The AMD output is preferred only where its runtime and virtual-package requirements are satisfied.
Use [rich platforms](../../workspace/multi_platform_configuration.md#declaring-virtual-packages-per-platform) to declare separate AMD and CPU solve targets, with AMD first for automatic selection on compatible machines.

See [Building Mojo CPU and AMD Packages](../mojo_accelerators.md) for a producer, a local channel, and an ordinary binary consumer.

## See Also

- [Mojo Pixi Basic](https://docs.modular.com/pixi/)
- [Modular Community Packages](https://github.com/modular/modular-community)
