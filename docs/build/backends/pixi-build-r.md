# pixi-build-r

The `pixi-build-r` backend is designed for building R packages using `R CMD INSTALL`. It automatically parses the `DESCRIPTION` file to extract metadata and dependencies, and detects whether native code compilation is needed.

!!! warning
    `pixi-build` is a preview flag, and will change until it is stabilized.
    This is why we require users to opt in to that feature by adding "pixi-build" to `workspace.preview`.

    ```toml
    [workspace]
    preview = ["pixi-build"]
    ```


## Overview

This backend automatically generates conda packages from R projects by:

- **DESCRIPTION parsing**: Reads package metadata, dependencies (`Imports`, `Depends`, `LinkingTo`), and license information from the standard R `DESCRIPTION` file
- **Automatic compiler detection**: Detects native code by checking for a `src/` directory or `LinkingTo` fields, and adds C, C++, and Fortran compilers automatically
- **Dependency mapping**: Converts R package names to conda names: CRAN packages become `r-<pkg>`, Bioconductor packages become `bioconductor-<pkg>`, with support for custom overrides
- **Cross-platform support**: Generates platform-appropriate build scripts for Linux, macOS, and Windows

## Basic Usage

To use the R backend in your `pixi.toml`, add it to your package's build configuration:

```toml
[workspace]
channels = ["https://prefix.dev/conda-forge"]
platforms = ["linux-64", "osx-arm64", "win-64"]
preview = ["pixi-build"]

[package]
name = "r-mypackage"
version = "1.0.0"

[package.build.backend]
name = "pixi-build-r"
channels = ["https://prefix.dev/conda-forge"]
```

Your R package should have a standard `DESCRIPTION` file in the project root:

```
Package: mypackage
Version: 1.0.0
Title: My R Package
Description: A short description of the package.
License: MIT
Imports:
    dplyr (>= 1.0),
    ggplot2
```

### Required Dependencies

The backend automatically includes the following dependencies:

- `r-base` - The R runtime (added to both host and run dependencies)

Dependencies listed in `Imports`, `Depends`, and `LinkingTo` fields of the `DESCRIPTION` file are automatically converted to conda packages and added to the recipe.

You can add additional dependencies to your [`host-dependencies`](https://pixi.sh/latest/build/dependency_types/) if needed:

```toml
[package.host-dependencies]
r-base = ">=4.1"
```

## Configuration Options

You can customize the R backend behavior using the `[package.build.config]` section in your `pixi.toml`. The backend supports the following configuration options:

### `extra-args`

- **Type**: `Array<String>`
- **Default**: `[]`
- **Target Merge Behavior**: `Overwrite` - Platform-specific args completely replace base args

Extra arguments to pass to `R CMD INSTALL`.

```toml
[package.build.config]
extra-args = ["--no-multiarch", "--no-test-load"]
```

For target-specific configuration, platform-specific args completely replace the base:

```toml
[package.build.config]
extra-args = ["--no-multiarch"]

[package.build.target.win-64.config]
extra-args = ["--no-multiarch", "--no-test-load"]
# Result for win-64: ["--no-multiarch", "--no-test-load"]
```

### `env`

- **Type**: `Map<String, String>`
- **Default**: `{}`
- **Target Merge Behavior**: `Merge` - Platform environment variables override base variables with same name, others are merged

Environment variables to set during the build process. These variables are available during `R CMD INSTALL`.

```toml
[package.build.config]
env = { R_KEEP_PKG_SOURCE = "yes" }
```

For target-specific configuration, platform environment variables are merged with base variables:

```toml
[package.build.config]
env = { COMMON_VAR = "base" }

[package.build.target.win-64.config]
env = { COMMON_VAR = "windows", WIN_SPECIFIC = "value" }
# Result for win-64: { COMMON_VAR = "windows", WIN_SPECIFIC = "value" }
```

--8<-- "docs/partials/build-config-env-expansion.md"

### `extra-input-globs`

- **Type**: `Array<String>`
- **Default**: `[]`
- **Target Merge Behavior**: `Overwrite` - Platform-specific globs completely replace base globs

Additional glob patterns to include as input files for the build process. These patterns are added to the default input globs that include R source files, documentation, and build-related files.

```toml
[package.build.config]
extra-input-globs = [
    "inst/**/*",
    "data/**/*",
    "vignettes/**/*"
]
```

### `compilers`

- **Type**: `Array<String>`
- **Default**: Auto-detected (see below)
- **Target Merge Behavior**: `Overwrite` - Platform-specific compilers completely replace base compilers

List of compilers to use for the build. By default, the backend auto-detects whether compilers are needed by checking for:

1. A `src/` directory in the package root
2. A `LinkingTo` field in the `DESCRIPTION` file

If either is found, compilers default to `["c", "cxx", "fortran"]`. Otherwise, no compilers are added.

```toml
[package.build.config]
compilers = ["c", "cxx"]  # Override auto-detection
```

For target-specific configuration, platform compilers completely replace the base configuration:

```toml
[package.build.config]
compilers = ["c"]

[package.build.target.win-64.config]
compilers = ["c", "cxx", "fortran"]
# Result for win-64: ["c", "cxx", "fortran"]
```

!!! info "Auto-Detection Behavior"
    Unlike the Python backend which defaults to no compilers, the R backend actively inspects your package structure. Packages with a `src/` directory or `LinkingTo` dependencies automatically get C, C++, and Fortran compilers. Pure R packages (no `src/`, no `LinkingTo`) get no compilers.

    You can override this by explicitly setting the `compilers` option:

    ```toml
    # Force no compilers even if src/ exists
    [package.build.config]
    compilers = []

    # Only use C compiler
    [package.build.config]
    compilers = ["c"]
    ```

!!! info "Comprehensive Compiler Documentation"
    For detailed information about available compilers, platform-specific behavior, and how conda-forge compilers work, see the [Compilers Documentation](../key_concepts/compilers.md).

### `channels`

- **Type**: `Array<String>`
- **Default**: `["conda-forge"]`
- **Target Merge Behavior**: `Overwrite` - Platform-specific channels completely replace base channels

Channels to use for resolving R package dependencies.

```toml
[package.build.config]
channels = ["conda-forge", "r"]
```

### `mapping`

- **Type**: `Map<String, String>`
- **Default**: `{}`
- **Target Merge Behavior**: `Merge` - Platform mappings are merged with base mappings, overriding keys with the same name

Custom mapping of R package names to conda package names. This allows you to override default naming conventions or map packages with non-standard names on Conda channels:

```toml
[package.build.config]
mapping = { Biobase = "bioconductor-biobase", my_pkg = "custom-conda-package" }

```

### `ignore-description-dependencies`

- **Type**: `Boolean`
- **Default**: `false`
- **Target Merge Behavior**: `Overwrite` - Target value overrides base value

When set to `true`, `pixi-build-r` does not automatically extract dependencies from the `DESCRIPTION` file. Only dependencies explicitly defined in the `pixi.toml` manifest (`host-dependencies`, `run-dependencies`) will be included in the generated recipe.

```toml
[package.build.config]
ignore-description-dependencies = true
```

## Dependency Handling

### Automatic Dependency Parsing

The backend reads dependencies from the `DESCRIPTION` file:

- **`Imports`** and **`Depends`** fields are added to both host and run dependencies
- **`LinkingTo`** fields are added to host dependencies only (compile-time headers)
- R version constraints are converted to conda format (e.g., `(>= 1.5)` becomes `>=1.5`)
- CRAN package names are converted to conda names with the `r-` prefix (e.g., `dplyr` becomes `r-dplyr`)
- Bioconductor package names are converted to conda names with the `bioconductor-` prefix (e.g., `Biobase` becomes `bioconductor-biobase`)
- A dependency already in `host-dependencies` or `run-dependencies`, as `r-<pkg>`, `bioconductor-<pkg>` or its mapped name, isn't added again from `DESCRIPTION`, so the manifest can set its version or name

### Bioconductor Support

Bioconductor packages are on the `bioconda` channel, named `bioconductor-<pkg>`.
When a dependency in `DESCRIPTION` is a Bioconductor package, `pixi-build-r` adds it as `bioconductor-<lowercase>` instead of `r-<lowercase>`.
It knows them from a built-in list of Bioconductor's software, annotation data, experiment data and workflow packages of recent releases, generated from Bioconductor's own package lists (`pixi run update-bioconductor-packages`).

These dependencies need `bioconda` among the workspace's channels:

```toml
[workspace]
channels = ["conda-forge", "bioconda"]
```

Without it, the build stops with an error that names the Bioconductor packages.

A Bioconductor package built from source is itself named `bioconductor-<lowercase>`, as every Bioconductor package declares `biocViews` in its `DESCRIPTION`, so its key in the manifest is that name:

```toml
[dependencies]
bioconductor-dnacopy = { git = "https://github.com/bioc/DNAcopy", branch = "RELEASE_3_22", package.build.backend.name = "pixi-build-r" }
```

If a package needs another name, set it with `mapping`.

### Built-in Packages

R's base packages (such as `stats`, `utils`, `base` and `methods`) come with `r-base`, so they are filtered out and not added as separate dependencies.
R's recommended packages (such as `MASS`, `Matrix` and `mgcv`) are separate packages on conda-forge (`r-mass`, `r-matrix`, `r-mgcv`), so they are added like any other dependency.

### Rust

Packages with Rust code declare it in `SystemRequirements`, as CRAN asks: `Cargo (Rust's package manager), rustc`.
When `SystemRequirements` mentions `cargo` or `rustc`, the backend adds `rust` to the build requirements, and on Windows also `rust-std-x86_64-pc-windows-gnu`, since R packages build their Rust code for the GNU target there.

### CMake and pkg-config

When `SystemRequirements` names `cmake`, the backend adds `cmake` to the build requirements, and when it names `pkg-config`, `pkg-config`, so the build doesn't depend on whichever of them is on the machine's `PATH`.

## Build Process

The R backend follows this build process:

1. **DESCRIPTION Parsing**: Reads package metadata and dependencies from the `DESCRIPTION` file
2. **Compiler Detection**: Auto-detects or uses configured compilers based on package structure
3. **Recipe Generation**: Creates a conda recipe with all dependencies converted to conda format
4. **Build Script**: Generates a platform-appropriate script that:
   - Prints R version information for debugging
   - Creates the R library directory
   - Runs `R CMD INSTALL --library=<library_dir> --no-lock <source_dir>`
5. **Package Creation**: Creates a platform-specific conda package

## Limitations

- Requires a standard R `DESCRIPTION` file in the project root
- The `DESCRIPTION` file must use the DCF (Debian Control File) format
- `Suggests` and `Enhances` dependencies are not automatically included
- Bioconductor dependencies are not supported
- License mapping from CRAN format to SPDX is best-effort

## See Also

- [Build Backends Overview](../backends.md) - Overview of all available build backends
- [Compilers](../key_concepts/compilers.md) - How pixi-build integrates with conda-forge's compiler infrastructure
- [CRAN](https://cran.r-project.org/) - The Comprehensive R Archive Network
