
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

This is an overview of the pixi manifest using `pixi-build`.

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_tomls/simple_pixi_build.toml:full"
```


Under the `[workspace]` section, you can specify properties like the name, channels, and platforms. This is currently an alias for `project`.

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
There are different build backends. Pixi backends can describe how to build a conda package, for a certain language or build tool.
In this example, we are using `pixi-build-python` backend in order to build a Python package.

```toml
--8<-- "docs/source_files/pixi_tomls/simple_pixi_build.toml:build-system"
```


We need to add our package `simple_python` as dependency to the workspace.

`pixi` also supports `git` dependencies, allowing you to specify a `branch`, `tag`, or `rev` to pin the dependency.
If none are specified, the latest commit on the default branch is used. The `subdirectory` is optional and specifies the location of the package within the repository.


```toml
--8<-- "docs/source_files/pixi_tomls/simple_pixi_build.toml:dependencies"
```

`simple_python` uses `hatchling` as Python build backend, so this needs to be mentioned in `host-dependencies`.

Python PEP517 backends like `hatchling` know how to build a Python package.
So `hatchling` creates a Python package, and `pixi-build-python` turns the Python package into a conda package.

Read up on host-dependencies in the [dependency types chapter](./dependency_types.md#host-dependencies)

```toml
--8<-- "docs/source_files/pixi_tomls/simple_pixi_build.toml:host-dependencies"
```

We add `rich` as a run dependency to the package. This is necessary because the package uses `rich` during runtime.
You can read up on run-dependencies in the [dependency types chapter](./dependency_types.md#dependencies-run-dependencies)

```toml
--8<-- "docs/source_files/pixi_tomls/simple_pixi_build.toml:run-dependencies"
```

## CLI Commands
Using the preview feature you can now build packages from source.

- `pixi build` has been added and will build a `.conda` file out of your package.
- Other commands like `pixi install` and `pixi run` automatically make use of the build feature when a `path`, `git` or `url` dependency is present.
