
## Introduction

Next to managing workflows and environments, pixi can also build packages.
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
    3. Recursive source dependencies are not supported. ( source dependencies that have source dependencies )
    4. Workspace dependencies cannot be inherited.

## Setting up the Manifest


```toml
--8<-- "docs/source_files/pixi_tomls/simple_pixi_build.toml:full"
```

Since the build feature is still in preview, you have to add "pixi-build" to `workspace.preview`.

```toml
--8<-- "docs/source_files/pixi_tomls/simple_pixi_build.toml:preview"
```

In `package` you specify properties specific to the package you want to build.

```toml
--8<-- "docs/source_files/pixi_tomls/simple_pixi_build.toml:package"
```

Packages are built by using build backends.
By specifying `package.build-system.build-backend` and `package.build-system.channels` you determine which backend is used and from which channel it will be downloaded.
In this example, we are using `pixi-build-python` in order to build a Python package.

```toml
--8<-- "docs/source_files/pixi_tomls/simple_pixi_build.toml:build-system"
```

1. Specifies workspace properties like the name, channels, and platforms. This is currently an alias for `project`.
2. Since the build feature is still in preview, you have to add "pixi-build" to `workspace.preview`.
3. We need to add our package as dependency to the workspace.
4. In `package` you specify properties specific to the package you want to build.
5. Packages are built by using build backends.
   By specifying `build-system.build-backend` and `build-system.channels` you determine which backend is used and from which channel it will be downloaded.
6. There are different build backends.
   Pixi backends can describe how to build a conda package, for a certain language or build tool.
   For example, `pixi-build-python`, allows building a Python package into a conda package.
7. `simple_python` uses `hatchling` as Python build backend so this needs to be mentioned in `host-dependencies`.
   Read up on host-dependencies in the [Dependency Types](./dependency_types.md#host-dependencies)
8. Python PEP517 backends like `hatchling` know how to build a Python package.
   So `hatchling` creates a Python package, and `pixi-build-python` turns the Python package into a conda package.

## CLI Commands
Using the preview feature you can now build packages from source.

- `pixi build` has been added and will build a `.conda` file out of your package.
- Other commands like `pixi install` and `pixi run` automatically make use of the build feature when a `path`, `git` or `url` dependency is present.
