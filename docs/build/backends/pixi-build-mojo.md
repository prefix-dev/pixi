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

Build CPU and AMD GPU variants with `mojo_accelerator`:

```toml
[workspace]
platforms = [{ platform = "linux-64", glibc = "2.34", archspec = "x86_64_v3" }]
preview = ["pixi-build"]

[workspace.build-variants]
mojo_accelerator = ["cpu", "gfx1103"]
```

Supported values are `cpu` and the following AMD GPU targets:

- `gfx90a`, `gfx942`, `gfx950`
- `gfx1030`, `gfx1033`
- `gfx1100`, `gfx1101`, `gfx1102`, `gfx1103`
- `gfx1150`, `gfx1151`, `gfx1152`
- `gfx1200`, `gfx1201`

Accelerator variants apply only to `linux-64` binaries (`bins`).
The variant is ignored for packages (`pkg`) and other platforms.
Other GPU targets, including CUDA, are not supported.
Without this variant, builds are CPU-only.

Both outputs share the package name, version, executable name, and build number.
The `mojo_accelerator` value changes the [build hash](https://rattler-build.prefix.dev/latest/variants/#package-hash-from-variant).

### Compilation

The backend passes `-D mojo_accelerator=<value>`, where `<value>` is the selected variant.
Your Mojo source can read this value with `get_defined_string["mojo_accelerator"]()` and use `comptime if` to compile either its CPU or GPU implementation.
This does not automatically turn CPU code into GPU code or select an accelerator at runtime.
CPU builds must exclude GPU API calls through a compile-time branch.
AMD builds also pass `--target-accelerator <value>` to target the selected GPU architecture.

Both builds target x86-64-v3, the [Modular runtime's minimum](https://mojolang.org/docs/requirements/#hardware), without host-specific optimizations.
Keep binary `extra-args` consistent with the selected target and package requirements.

The backend supplies C/C++ compilers, `patchelf`, and a sysroot matching the declared glibc version.
No manual compiler paths or sysroot pins are needed.
Pin `mojo-compiler >=1.2` in `[package.build-dependencies]` to select the compiler version.
Host and runtime requirements follow that selection.

### Runtime and selection

Both builds require glibc 2.34 or newer, x86-64-v3, and matching `mojo-compiler` and `max-core >=26.7` runtimes.
AMD builds also require `__amdgpu` and an exact `__amdgpu_arch` match for the selected target.
For example, `gfx1103` requires `__amdgpu_arch ==11.0.3`, not that architecture or newer.
The minimum `hip-runtime-amd` version is 7.0 for `gfx950` and 6.3.3 for all other supported AMD targets.

Pixi tries the AMD platform first when its built-in declarations match the host.
If its locked requirements, including the exact ISA dependency, don't fit, Pixi tries the CPU platform.
Define AMD and CPU platforms in [`workspace.platforms`](../../workspace/multi_platform_configuration.md#declaring-virtual-packages-per-platform), with AMD first.
See [Which platform gets installed?](../../workspace/multi_platform_configuration.md#which-platform-gets-installed) for the selection rules.

See [Building Mojo CPU and AMD Packages](../mojo_accelerators.md) for a complete example.

## See Also

- [Mojo Pixi Basic](https://docs.modular.com/pixi/)
- [Modular Community Packages](https://github.com/modular/modular-community)
