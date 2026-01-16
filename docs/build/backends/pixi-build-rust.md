# pixi-build-rust

The `pixi-build-rust` backend is designed for building Rust projects using [Cargo](https://doc.rust-lang.org/cargo/), Rust's native build system and package manager. It provides seamless integration with Pixi's package management workflow while maintaining cross-platform compatibility.

!!! warning
    `pixi-build` is a preview feature, and will change until it is stabilized.
    This is why we require users to opt in to that feature by adding "pixi-build" to `workspace.preview`.

    ```toml
    [workspace]
    preview = ["pixi-build"]
    ```


## Overview

This backend automatically generates conda packages from Rust projects by:

- **Using Cargo**: Leverages Rust's native build system for compilation and installation
- **Cargo.toml Integration**: Automatically reads package metadata (name, version, description, license, etc.) from your `Cargo.toml` file when not specified in `pixi.toml`
- **Cross-platform support**: Works consistently across Linux, macOS, and Windows
- **Optimization support**: Automatically detects and integrates with `sccache` for faster compilation
- **OpenSSL integration**: Handles OpenSSL linking when available in the environment

## Basic Usage

To use the Rust backend in your `pixi.toml`, add it to your package's build configuration:

```toml
[package]
name = "rust_package"
version = "0.1.0"

[package.build]
backend = { name = "pixi-build-rust", version = "*" }
channels = ["https://prefix.dev/conda-forge"]

```

### Automatic Metadata Detection

The backend will automatically read metadata from your `Cargo.toml` file to populate package information **that is not** explicitly defined in your `pixi.toml`.
This includes:

- **Package name and version**: Automatically used if not specified in `pixi.toml`
- **License**: Extracted from `Cargo.toml` license field
- **Description**: Uses the description from `Cargo.toml`
- **Homepage**: From the homepage field in `Cargo.toml`
- **Repository**: From the repository field in `Cargo.toml`
- **Documentation**: From the documentation field in `Cargo.toml`

For example, if your `Cargo.toml` contains:

```toml
[package]
name = "my-rust-tool"
version = "1.0.0"
description = "A useful Rust command-line tool"
license = "MIT"
homepage = "https://github.com/user/my-rust-tool"
repository = "https://github.com/user/my-rust-tool"
```

You can create a minimal `pixi.toml`:

```toml
[package.build]
backend = { name = "pixi-build-rust", version = "*" }
channels = ["https://prefix.dev/conda-forge"]
```

The backend will automatically use the metadata from `Cargo.toml` to generate a complete conda package.

??? warning "It still requires you to specify the `name` and `version`"
    We're in the process of making this optional in `pixi`, but for now, you need to specify them explicitly.
    This is the tracking issue to fix this in [Pixi](https://github.com/prefix-dev/pixi/issues/4317)


### Required Dependencies

The backend automatically includes the following build tools:

- `rust` - The Rust compiler and toolchain
- `cargo` - Rust's package manager (included with rust)

You can add these to your [`build-dependencies`](https://pixi.sh/latest/build/dependency_types/) if you need specific versions:

```toml
[package.build-dependencies]
rust = "1.70"
```

## Configuration Options

You can customize the Rust backend behavior using the `[package.build.config]` section in your `pixi.toml`. The backend supports the following configuration options:

### `extra-args`

- **Type**: `Array<String>`
- **Default**: `[]`
- **Target Merge Behavior**: `Overwrite` - Platform-specific arguments completely replace base arguments

Additional command-line arguments to pass to the `cargo install` command. These arguments are appended to the cargo command that builds and installs your project.

```toml
[package.build.config]
extra-args = [
    "--features", "serde,tokio",
    "--bin", "my-binary"
]
```

For target-specific configuration, platform arguments completely replace the base configuration:

```toml
[package.build.config]
extra-args = ["--release"]

[package.build.target.linux-64.config]
extra-args = ["--features", "linux-specific", "--target", "x86_64-unknown-linux-gnu"]
# Result for linux-64: ["--features", "linux-specific", "--target", "x86_64-unknown-linux-gnu"]
```

### `env`

- **Type**: `Map<String, String>`
- **Default**: `{}`
- **Target Merge Behavior**: `Merge` - Platform environment variables override base variables with same name, others are merged

Environment variables to set during the build process. These variables are available during compilation.

```toml
[package.build.config]
env = { RUST_LOG = "debug", CARGO_PROFILE_RELEASE_LTO = "true" }
```

For target-specific configuration, platform environment variables are merged with base variables:

```toml
[package.build.config]
env = { RUST_LOG = "info", COMMON_VAR = "base" }

[package.build.target.linux-64.config]
env = { COMMON_VAR = "linux", CARGO_PROFILE_RELEASE_LTO = "true" }
# Result for linux-64: { RUST_LOG = "info", COMMON_VAR = "linux", CARGO_PROFILE_RELEASE_LTO = "true" }
```

### `debug-dir`

The backend always writes JSON-RPC request/response logs and the generated intermediate recipe to the `debug` subdirectory inside the work directory (for example `<work_directory>/debug`). The deprecated `debug-dir` configuration option is ignored; when present a warning is emitted so you can safely remove the setting.

### `extra-input-globs`

- **Type**: `Array<String>`
- **Default**: `[]`
- **Target Merge Behavior**: `Overwrite` - Platform-specific globs completely replace base globs

Additional glob patterns to include as input files for the build process. These patterns are added to the default input globs that include Rust source files (`**/*.rs`), Cargo configuration files (`Cargo.toml`, `Cargo.lock`), build scripts (`build.rs`), and other build-related files.

```toml
[package.build.config]
extra-input-globs = [
    "assets/**/*",
    "migrations/*.sql",
    "*.md"
]
```

For target-specific configuration, platform-specific globs completely replace the base:

```toml
[package.build.config]
extra-input-globs = ["*.txt"]

[package.build.target.linux-64.config]
extra-input-globs = ["*.txt", "*.so", "linux-configs/**/*"]
# Result for linux-64: ["*.txt", "*.so", "linux-configs/**/*"]
```

### `ignore-cargo-manifest`

- **Type**: `Boolean`
- **Default**: `false`
- **Target Merge Behavior**: `Overwrite` - Platform-specific value overrides base value if set

When set to `true`, disables automatic metadata extraction from `Cargo.toml`.
The backend will only use metadata explicitly defined in your `pixi.toml` file, ignoring any information from the Cargo manifest.

```toml
[package.build.config]
ignore-cargo-manifest = true
```

This is useful when:

- You want to explicitly control all package metadata through `pixi.toml`
- The `Cargo.toml` contains metadata that conflicts with your conda package requirements
- When using the `Cargo.toml` results in an error that you cannot resolve.

For target-specific configuration:

```toml
[package.build.config]
ignore-cargo-manifest = false

[package.build.target.linux-64.config]
ignore-cargo-manifest = true
# Result for linux-64: Cargo.toml metadata will be ignored
```

### `compilers`

- **Type**: `Array<String>`
- **Default**: `["rust"]`
- **Target Merge Behavior**: `Overwrite` - Platform-specific compilers completely replace base compilers

List of compilers to use for the build. The backend automatically generates appropriate compiler dependencies using conda-forge's compiler infrastructure.

```toml
[package.build.config]
compilers = ["rust", "c", "cxx"]
```

For target-specific configuration, platform compilers completely replace the base configuration:

```toml
[package.build.config]
compilers = ["rust"]

[package.build.target.linux-64.config]
compilers = ["rust", "c", "cxx"]
# Result for linux-64: ["rust", "c", "cxx"]
```

!!! info "Comprehensive Compiler Documentation"
    For detailed information about available compilers, platform-specific behavior, and how conda-forge compilers work, see the [Compilers Documentation](../key_concepts/compilers.md).


## Build Process

The Rust backend follows this build process:

1. **Environment Setup**: Configures OpenSSL paths if available in the environment
2. **Compiler Caching**: Sets up `sccache` as `RUSTC_WRAPPER` if available for faster compilation
3. **Build and Install**: Executes `cargo install` with the following default options:
   - `--locked`: Use the exact versions from `Cargo.lock`
   - `--root "$PREFIX"`: Install to the conda package prefix
   - `--path .`: Install from the current source directory
   - `--no-track`: Don't track installation metadata
   - `--force`: Force installation even if already installed
4. **Cache Statistics**: Displays `sccache` statistics if available

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

## Limitations

- Currently, uses `cargo install` which builds in release mode by default
- No support for custom Cargo profiles in the build configuration
- Limited workspace support for multi-crate projects

## See Also

- [Cargo Documentation](https://doc.rust-lang.org/cargo/) - Official Cargo documentation
- [The Rust Programming Language](https://doc.rust-lang.org/book/) - Official Rust book
- [sccache](https://github.com/mozilla/sccache) - Shared compilation cache for Rust
