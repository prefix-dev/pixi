# Compilers in pixi-build

Some `pixi-build` backends support configurable compiler selection through the `compilers` configuration option. This feature integrates with conda-forge's compiler infrastructure to provide cross-platform, ABI-compatible builds.

!!! warning
    `pixi-build` is a preview feature, and will change until it is stabilized.
    This is why we require users to opt in to that feature by adding "pixi-build" to `workspace.preview`.

    ```toml
    [workspace]
    preview = ["pixi-build"]
    ```

## How Conda-forge Compilers Work

Understanding conda-forge's compiler system is essential for effectively using `pixi-build` compiler configuration.

### Compiler Selection and Platform Resolution

When you specify `compilers = ["c", "cxx"]` in your `pixi-build` configuration, the backend automatically selects the appropriate platform-specific compiler packages based on your target platform and build variants.
If you are cross-compiling the target platform will be the platform you are compiling for.
Otherwise, it the target platform is your current platform.

If your target platform is `amd64`, this will result in the following packages to be selected by default.

| Compiler | Linux | macOS | Windows |
|----------|-------|--------|---------|
| `c` | `gcc_linux-64` | `clang_osx-64` | `vs2019_win-64` |
| `cxx` | `gxx_linux-64` | `clangxx_osx-64` | `vs2019_win-64` |
| `fortran` | `gfortran_linux-64` | `gfortran_osx-64` | `vs2019_win-64` |

### Build Variants and Compiler Selection

Compiler selection works through a build variant system. Build variants allow you to specify different versions or types of compilers for your builds, creating a build matrix that can target multiple compiler configurations.

### Overriding Compilers in Pixi Workspaces

Pixi workspaces provide powerful mechanisms to override compiler variants through build variant configuration.
This allows users to customize compiler selection without modifying individual package recipes.

To overwrite the default C compiler you can modify your `pixi.toml` file in the workspace root:

```toml
# pixi.toml
[workspace.build-variants]
c_compiler = ["clang"]
c_compiler_version = ["11.4"]
```

To overwrite the c/cxx compiler specifically for Windows you can use the `workspace.target` section to specify platform-specific compiler variants:

```toml
# pixi.toml
[workspace.target.win.build-variants]
c_compiler = ["vs2022"]
cxx_compiler = ["vs2022"]
```

Or

```toml
[workspace.target.win.build-variants]
c_compiler = ["vs"]
cxx_compiler = ["vs"]
c_compiler_version = ["2022"]
cxx_compiler_version = ["2022"]
```

#### How Compilers Are Selected

When you specify `compilers = ["c"]` in your pixi-build configuration, the system doesn't directly install a package named "c". Instead, it uses a **variant system** to determine the exact compiler package for your platform.

1. **Determine which compilers to add**

   If you specified the compiler in the configuration, it will use that.
   If the configuration has this entry `compilers = ["c"]`, the C compiler will be requested.
   If there's no compiler configuration, the [default](./compilers.md#backend-specific-defaults) of the backend will be used.

2. **For each compiler, determine the variants to take into account**

   The variant names follow the pattern `{language}_compiler` and `{language}_compiler_version`.
   In our example that would lead to `c_compiler` and `c_compiler_version`.

3. **For each variant combination, create an output**

   Each variant can have multiple values and each combination of these values are outputs that can be selected.
   For example with the following example multiple `gcc` versions could be used to build this package.

   ```toml
   [workspace.build-variants]
   c_compiler = ["gcc"]
   c_compiler_version = ["11.4", "14.0"]
   ```

   If `{language}_compiler_version` is not set, then there's no constraint on the compiler version.

   If `{language}_compiler` is not set, the build-backends set default values for certain languages:

   - c: `gcc` on Linux, `clang` on osx and `vs2017` on Windows
   - cxx: `gxx` on Linux, `clangxx` on osx and `vs2017` on Windows
   - fortran: `gfortran` on Linux, `gfortran` on osx and `vs2017` on Windows
   - rust: `rust`

4. **Request a package for each output**

   For each output a package will be requested as build dependency with the following pattern `{compiler}_{target_platform} {compiler_version}`.
   `compiler` and `compiler_version` has been determined in the step before.
   `target_platform` is the platform you are compiling for, if you are cross compiling the target platform would differ from your current platform.

   In our example we would create two outputs.
   If we build on linux-64, one output would request `gcc_linux-64 11.4` and one would request `gcc_linux-64 14.0`



## Available Compilers

Which compilers are available depends on the channels you target but through the conda-forge infrastructure the following compilers are generally available across all platforms.
The table below lists the core compilers, specialized compilers, and some backend language-specific compilers that can be configured in `pixi-build`.

### Core Compilers

| Compiler | Description | Platforms |
|----------|-------------|-----------|
| `c` | C compiler | Linux (gcc), macOS (clang), Windows (vs2019) |
| `cxx` | C++ compiler | Linux (gxx), macOS (clangxx), Windows (vs2019) |
| `fortran` | Fortran compiler | Linux (gfortran), macOS (gfortran), Windows (vs2019) |
| `rust` | Rust compiler | All platforms |
| `go` | Go compiler | All platforms |

### Specialized Compilers

| Compiler | Description | Platforms |
|----------|-------------|-----------|
| `cuda` | NVIDIA CUDA compiler | Linux, Windows, (limited macOS) |

## Backend-Specific Defaults

Only certain `pixi-build` backends support the `compilers` configuration option. Each supporting backend has sensible defaults based on the typical requirements for that language ecosystem:

| Backend | Compiler Support | Default Compilers | Rationale |
|---------|------------------|-------------------|-----------|
| **[pixi-build-cmake](../backends/pixi-build-cmake.md#compilers)** | ✅ **Supported** | `["cxx"]` | Most CMake projects are C++ |
| **[pixi-build-rust](../backends/pixi-build-rust.md#compilers)** | ✅ **Supported** | `["rust"]` | Rust projects need the Rust compiler |
| **[pixi-build-python](../backends/pixi-build-python.md#compilers)** | ✅ **Supported** | `[]` | Pure Python packages typically don't need compilers |
| **[pixi-build-mojo](../backends/pixi-build-mojo.md#compilers)** | ✅ **Supported** | `[]` | `mojo-compiler` must be specified in the `package.*-dependencies` manually. |
| **pixi-build-rattler-build** | ❌ **Not Supported** | N/A | Uses direct `recipe.yaml` - configure compilers directly in recipe |

!!! info "Adding Compiler Support to Other Backends"
    Backend developers can add compiler configuration support by implementing the `compilers` field in their backend configuration and integrating with the shared compiler infrastructure in `pixi-build-backend`.

## Configuration Examples

To configure compilers in your `pixi-build` project, you can use the `compilers` configuration option in your `pixi.toml` file. Below are some examples of how to set up compiler configurations for different scenarios.

!!! note "Backend Support"
Compiler configuration is only available in backends that have specifically implemented this feature. Not all backends support the `compilers` configuration option. Check your backend's documentation to see if it supports compiler configuration.

### Basic Compiler Configuration

```toml
# Use default compilers for the backend
[package.build.config]
# No compilers specified - uses backend defaults

# Override with specific compilers
[package.build.config]
compilers = ["c", "cxx", "fortran"]
```

### Platform-Specific Compiler Configuration

```toml
# Base configuration for most platforms
[package.build.config]
compilers = ["cxx"]

# Linux needs additional CUDA support
[package.build.target.linux-64.config]
compilers = ["cxx", "cuda"]

# Windows needs additional C compiler for some dependencies
[package.build.target.win-64.config]
compilers = ["c", "cxx"]
```
