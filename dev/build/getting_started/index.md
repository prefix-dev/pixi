Next to managing workflows and environments, Pixi can also build packages. This is useful for the following reasons:

- Building and uploading a package to a conda channel
- Allowing users to directly depend on the source and build it automatically
- Managing multiple packages in a workspace

We've been working to support these use-cases with the `build` feature in pixi. The vision is to enable building of packages from source, for any language, on any platform.

Known limitations

Currently, the `build` feature has a number of limitations:

1. Limited set of [build-backends](https://github.com/prefix-dev/pixi-build-backends).
1. Build-backends are probably missing a lot of parameters/features.
1. Recursive source dependencies are not supported. (source dependencies that have source dependencies)
1. Workspace dependencies cannot be inherited.

## Setting up the Manifest

This is an overview of the Pixi manifest using the `pixi-build` feature.

A more in-depth overview of what is available in the `[package]` part of the manifest can be found in the [Manifest Reference](../../reference/pixi_manifest/#the-package-section).

pixi.toml

```toml
### Specifies properties for the whole workspace ###
[workspace]
preview = ["pixi-build"]
channels = ["https://prefix.dev/conda-forge"]
platforms = ["win-64", "linux-64", "osx-arm64", "osx-64"]
[tasks]
start = "rich-example-main"
[dependencies]
python_rich = { path = "." }
### Specify the package properties ###
[package]
name = "python_rich"
version = "0.1.0"
# We are using `pixi-build-python` in order to build a Python package
[package.build.backend]
name = "pixi-build-python"
version = "==0.3.2"
# The Python package `python_rich` uses `hatchling` as Python build backend
[package.host-dependencies]
hatchling = "*"
# The Python package `python_rich` has a run dependency on `rich`
[package.run-dependencies]
rich = "13.9.*"

```

Under the `[workspace]` section, you can specify properties like the name, channels, and platforms. This is currently an alias for `[project]`.

Since the build feature is still in preview, you have to add "pixi-build" to `workspace.preview`.

```toml
[workspace]
preview = ["pixi-build"]

```

In `package` you specify properties specific to the package you want to build.

```toml
[package]
name = "python_rich"
version = "0.1.0"

```

Packages are built by using build backends. By specifying `package.build.backend` and `package.build.channels` you determine which backend is used and from which channel it will be downloaded.

There are [different build backends available](https://prefix-dev.github.io/pixi-build-backends/).

Pixi backends describe how to build a conda package, for a certain language or build tool. In this example, we are using `pixi-build-python` backend in order to build a Python package.

```toml


```

We need to add our package `python_rich` as source dependency to the workspace.

```toml
[dependencies]
python_rich = { path = "." }

```

`python_rich` uses `hatchling` as Python build backend, so this needs to be mentioned in `host-dependencies`.

Python PEP517 backends like `hatchling` know how to build a Python package. So `hatchling` creates a Python package, and `pixi-build-python` turns the Python package into a conda package.

Read up on host-dependencies in the [dependency types chapter](../dependency_types/#host-dependencies)

```toml
[package.host-dependencies]
hatchling = "*"

```

We add `rich` as a run dependency to the package. This is necessary because the package uses `rich` during runtime. You can read up on run-dependencies in the [dependency types chapter](../dependency_types/#dependencies-run-dependencies)

```toml
[package.run-dependencies]
rich = "13.9.*"

```

## CLI Commands

Using the preview feature you can now build packages from source.

- `pixi build` has been added and will build a `.conda` file out of your package.
- Other commands like `pixi install` and `pixi run` automatically make use of the build feature when a `path`, `git` or `url` dependency is present.
