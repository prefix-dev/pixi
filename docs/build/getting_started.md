
Next to managing workflows and environments, Pixi can also build packages.
This is useful for the following reasons:

- Building and uploading a package to a conda channel
- Allowing users to directly depend on the source and build it automatically
- Managing multiple packages in a workspace

We've been working to support these use-cases with the `build` feature in pixi.
The vision is to enable building of packages from source, for any language, on any platform.


!!! note "Known limitations"
    Currently, the `build` feature has a number of limitations:

    1. Limited set of [build-backends](https://github.com/prefix-dev/pixi-build-backends).
    2. Build-backends are probably missing a lot of parameters/features.
    3. Recursive source dependencies are not supported. (source dependencies that have source dependencies)
    4. Workspace dependencies cannot be inherited.

## Setting up the Manifest

This is an overview of the Pixi manifest using the `pixi-build` feature.

A more in-depth overview of what is available in the `[package]` part of the manifest can be found in the [Manifest Reference](../reference/pixi_manifest.md#the-package-section).

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/pixi_build/getting_started/pixi.toml:full"
```

Under the `[workspace]` section, you can specify properties like the name, channels, and platforms. This is currently an alias for `[project]`.

Since the build feature is still in preview, you have to add "pixi-build" to `workspace.preview`.

```toml
--8<-- "docs/source_files/pixi_workspaces/pixi_build/getting_started/pixi.toml:preview"
```

In `package` you specify properties specific to the package you want to build.

```toml
--8<-- "docs/source_files/pixi_workspaces/pixi_build/getting_started/pixi.toml:package"
```

Packages are built by using build backends.
By specifying `package.build.backend` and `package.build.channels` you determine which backend is used and from which channel it will be downloaded.

There are [different build backends available](https://prefix-dev.github.io/pixi-build-backends/). 

Pixi backends describe how to build a conda package, for a certain language or build tool.
In this example, we are using `pixi-build-python` backend in order to build a Python package.

```toml
--8<-- "docs/source_files/pixi_workspaces/pixi_build/getting_started/pixi.toml:build-system"
```

We need to add our package `python_rich` as source dependency to the workspace.

```toml
--8<-- "docs/source_files/pixi_workspaces/pixi_build/getting_started/pixi.toml:dependencies"
```

`python_rich` uses `hatchling` as Python build backend, so this needs to be mentioned in `host-dependencies`.

Python PEP517 backends like `hatchling` know how to build a Python package.
So `hatchling` creates a Python package, and `pixi-build-python` turns the Python package into a conda package.

Read up on host-dependencies in the [dependency types chapter](./dependency_types.md#host-dependencies)

```toml
--8<-- "docs/source_files/pixi_workspaces/pixi_build/getting_started/pixi.toml:host-dependencies"
```

We add `rich` as a run dependency to the package. This is necessary because the package uses `rich` during runtime.
You can read up on run-dependencies in the [dependency types chapter](./dependency_types.md#dependencies-run-dependencies)

```toml
--8<-- "docs/source_files/pixi_workspaces/pixi_build/getting_started/pixi.toml:run-dependencies"
```

## CLI Commands
Using the preview feature you can now build packages from source.

- `pixi build` has been added and will build a `.conda` file out of your package.
- Other commands like `pixi install` and `pixi run` automatically make use of the build feature when a `path`, `git` or `url` dependency is present.
