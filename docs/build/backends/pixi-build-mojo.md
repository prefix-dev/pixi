# pixi-build-mojo

The `pixi-build-mojo` backend is designed for building Mojo projects. It provides seamless integration with Pixi's package management workflow.

!!! warning
    `pixi-build` is a preview feature, and will change until it is stabilized.
    This is why we require users to opt in to that feature by adding "pixi-build" to `workspace.preview`.

    ```toml
    [workspace]
    preview = ["pixi-build"]
    ```

## Overview

This backend automatically generates conda packages from Mojo projects.

The generated packages can be installed into local envs for development, or packaged for distribution.

### Auto-derive of pkg and bin

The Mojo backend includes auto-discovery of your project structure and will derive the following:

- **Binaries**: Automatically searches for `main.mojo` or `main.🔥` in:
  - `<project_root>/main.mojo`
- **Packages**: Automatically searches for directories with `__init__.mojo` or `__init__.🔥` in:
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
backend = { name = "pixi-build-mojo", version = "0.1.*" }

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

The backend always writes JSON-RPC request/response logs and the generated intermediate recipe to the `debug` subdirectory inside the work directory (for example `<work_directory>/debug`). The deprecated `debug-dir` configuration option is ignored; if present, the backend emits a warning to highlight that it no longer has any effect.

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

List of binary configurations to build. The created binary will be placed in the `$PREFIX/bin` dir and will be in the path after running `pixi install` assuming the package is listed as a dependency as in the example above. `pixi build` will create a conda package that includes the binary.

**Auto-derive behavior:**
- If `bins` is not specified, pixi-build-mojo will search for a `main.mojo` or `main.🔥` file in the project root
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
- For the first binary, searches for `main.mojo` or `main.🔥` in the project root
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
- If `pkg` is not specified, pixi-build-mojo will search for a directory containing `__init__.mojo` or `__init__.🔥` in the following order:
  1. `<project_root>/<project_name>/`
  2. `<project_root>/src/`
- If found, it creates a package with the name set to the project name
- If no valid package directory is found, no package is built
- If a binary is manually configured, a pkg will not be auto-derived and must be manually specified.
- If a binary is also auto-derive, a pkg will not be generated and must be manually specified

#### `pkg.name`

- **Type**: `String`
- **Default**: Project name (with dashes converted to underscores)

The name to give the Mojo package. The `.mojopkg` suffix will be added automatically. If not specified, defaults to the project name.

```toml
[package.build.config.pkg]
name = "greetings"
```

#### `pkg.path`

- **Type**: `String` (path)
- **Default**: Auto-derive

The path to the directory that constitutes the package. If not specified, searches for a directory with `__init__.mojo` or `__init__.🔥` as described above.

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

## See Also

- [Mojo Pixi Basic](https://docs.modular.com/pixi/)
- [Modular Community Packages](https://github.com/modular/modular-community)
