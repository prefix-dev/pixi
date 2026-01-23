# Rattler-Build Backend

The `pixi-build-rattler-build` backend enables building conda packages using rattler-build recipes.
This backend is designed for projects that either have existing recipe.yaml files or where customization is necessary that isn't possible with the currently available backends.

!!! warning
    `pixi-build` is a preview feature, and will change until it is stabilized.
    This is why we require users to opt in to that feature by adding "pixi-build" to `workspace.preview`.

    ```toml
    [workspace]
    preview = ["pixi-build"]
    ```


## Overview

The rattler-build backend:

- Uses existing `recipe.yaml` files as build manifests
- Supports all standard rattler-build recipe features and selectors
- Handles dependency resolution and virtual package detection automatically
- Can build multiple outputs from a single recipe

## Usage

To use the rattler-build backend in your `pixi.toml`, specify it in your build system configuration:

```toml
[package]
name = "rattler_build_package"
version = "0.1.0"

[package.build]
backend = { name = "pixi-build-rattler-build", version = "*" }
channels = ["https://prefix.dev/conda-forge"]
```

The backend expects a rattler-build recipe file in one of these locations (searched in order):

1. `recipe.yaml` or `recipe.yml` in the same directory as the package manifest
2. `recipe/recipe.yaml` or `recipe/recipe.yml` in a subdirectory of the package manifest

If the package is defined in the same location as the workspace, it is heavily encouraged to place the recipe file in its own directory `recipe`.
Learn more about the `rattler-build`, and its recipe format in its [high level overview](https://rattler.build/latest/highlevel).

!!! warning
    If you expect your build script to be compatible with incremental compilation
    (re-using files from previous builds to speed-up future builds),
    you must ensure that the build directory for these files is set outside of the
    root directory in order to enable the incremental compilation.
    This is because we use a clean root directory for each build,
    to ensure compatibility with recipes which make that assumption.
    
    In practice, this may look like changing directory to `../build_dir` in your
    build script before commencing the build, or passing `../build_dir` as an
    argument to your build system.

## Specifying dependencies

We only allow source dependencies (workspace packages) in project manifest.
Binary dependencies are not allowed in the project manifest when using `pixi-build-rattler-build`.
This is intentional because:

1. The rattler-build recipe is the source of truth for binary dependencies. It already
   specifies exact versions, build variants, and whether dependencies go in build/host/run.

2. Allowing binary dependencies in both places would create duplication and potential
   conflicts (e.g., recipe says "python >=3.10" but project model says "python >=3.9").

3. Source dependencies are different - they represent workspace packages built from local
   source. The recipe can reference them by name, but can't know their workspace paths.
   The project model provides this mapping.

This way, the recipe maintains full control over binary dependencies while the project
model only provides the workspace structure information that the recipe cannot know.

To specify source dependencies, add them to `build-dependencies`, `host-dependencies` or `run-dependencies` in the package manifest:

```toml title="pixi.toml"
[package.build-dependencies]
a = { path = "../a" }
```

## Configuration Options

The rattler-build backend supports the following TOML configuration options:

### `experimental`

- **Type**: `Boolean`
- **Default**: `false`
- **Target Merge Behavior**: Not allowed - must be set at root level only

Enables experimental features in rattler-build. This is required for certain advanced features like the `cache:` functionality for multi-output recipes.

```toml
[package.build.config]
experimental = true
```

Note: This option cannot be set in target-specific configurations. It must be set at the root `[package.build.config]` level only.

### `debug-dir`

The backend always writes JSON-RPC request/response logs and the generated intermediate recipe to the `debug` subdirectory inside the work directory (for example `<work_directory>/debug`). The deprecated `debug-dir` configuration option is ignored; if it is still set in a manifest the backend emits a warning to make the change explicit.

### `extra-input-globs`

- **Type**: `Array<String>`
- **Default**: `[]`
- **Target Merge Behavior**: `Overwrite` - Platform-specific globs completely replace base globs

Additional glob patterns to include as input files for the build process. These patterns are added to the default input globs that are determined from the recipe sources and package directory structure.

```toml
[package.build.config]
extra-input-globs = [
    "patches/**/*",
    "scripts/*.sh",
    "*.md"
]
```

For target-specific configuration, platform-specific globs completely replace the base:

```toml
[package.build.config]
extra-input-globs = ["*.yaml", "*.md"]

[package.build.target.linux-64.config]
extra-input-globs = ["*.yaml", "*.md", "*.sh", "patches-linux/**/*"]
# Result for linux-64: ["*.yaml", "*.md", "*.sh", "patches-linux/**/*"]
```

## Build Process

The rattler-build backend follows this build process:

1. **Recipe Discovery**: Locates the `recipe.yaml` file in standard locations
2. **Dependency Resolution**: Resolves build, host, and run dependencies from conda channels and workspace
3. **Virtual Package Detection**: Automatically detects system virtual packages
4. **Build Execution**: Runs the build script specified in the recipe
5. **Package Creation**: Creates conda packages according to the recipe specification


## Limitations

- Requires an existing rattler-build recipe file - cannot infer build instructions automatically
- Build configuration is primarily controlled through the recipe file rather than `pixi.toml`
- Cannot specify binary dependencies in the manifest
