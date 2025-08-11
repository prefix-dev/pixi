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

This is an overview of the Pixi manifest using `pixi-build`.

pixi.toml

```toml
# Specifies properties for the whole workspace
[workspace]
preview = ["pixi-build"]
channels = ["https://prefix.dev/conda-forge"]
platforms = ["win-64", "linux-64", "osx-arm64", "osx-64"]
# There can be multiple packages in a workspace
# In `package` you specify properties specific to the package
[package]
name = "python_rich"
version = "0.1.0"
# Here the build system of the package is specified
# We are using `pixi-build-python` in order to build a Python package
[package.build]
backend = { name = "pixi-build-python", version = "==0.3.2" }
# The Python package `python_rich` uses `hatchling` as Python build backend
[package.host-dependencies]
hatchling = "*"
# We add our package as dependency to the workspace
[dependencies]
# We can get source dependencies from a path on our file system
python_rich_by_folder = { path = "./recipe_by_folder" }
# as well as from a git repository
boost-check = { git = "https://github.com/wolfv/pixi-build-examples", branch = "main", subdirectory = "boost-check" }
# If the directory contains a `pixi.toml`, `pixi-build` will be used to build the package
python_rich = { path = "./python_rich" }
# if the directory contains a `recipe.yaml`, `rattler-build` will be used.
python_rich_by_file = { path = "./recipe_by_file/recipe.yaml" }
[package.run-dependencies]
rich = ">=13.9.4,<14"

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

Packages are built by using build backends. By specifying `package.build-system.build-backend` and `package.build-system.channels` you determine which backend is used and from which channel it will be downloaded. There are different build backends. Pixi backends can describe how to build a conda package, for a certain language or build tool. In this example, we are using `pixi-build-python` backend in order to build a Python package.

```toml
[package.build]
backend = { name = "pixi-build-python", version = "==0.3.2" }

```

We need to add our package `python_rich` as source dependency to the workspace.

Hint

Pixi dependencies fall into two main categories: `binary` and `source` dependencies. `binary` dependencies are pre-built packages, while `source` dependencies are source code that needs to be built.

Source dependencies can be specified either by providing a local path to the directory containing the package or a `git` dependency. When using `git`, you can optionally define a `branch`, `tag`, or `rev` to pin the dependency. If none are specified, the latest commit on the default branch is used. Additionally, a `subdirectory` can be specified to indicate the package’s location within the repository.

Using git SSH URLs

When using SSH URLs in git dependencies, make sure to have your SSH key added to your SSH agent. You can do this by running `ssh-add` which will prompt you for your SSH key passphrase. Make sure that the `ssh-add` agent or service is running and you have a generated public/private SSH key. For more details on how to do this, check the [Github SSH documentation](https://docs.github.com/en/authentication/connecting-to-github-with-ssh/generating-a-new-ssh-key-and-adding-it-to-the-ssh-agent).

Source dependencies are defined in one of two ways:

- `Pixi`-based dependencies are built using the backend specified in the `[package.build]` section of pixi.toml.
- `rattler-build`-based dependencies are built using a `recipe.yaml` file. You can specify the path to the folder containing the recipe file, or the path to the `recipe.yaml` file itself.

```toml
# We add our package as dependency to the workspace
[dependencies]
# We can get source dependencies from a path on our file system
python_rich_by_folder = { path = "./recipe_by_folder" }
# as well as from a git repository
boost-check = { git = "https://github.com/wolfv/pixi-build-examples", branch = "main", subdirectory = "boost-check" }
# If the directory contains a `pixi.toml`, `pixi-build` will be used to build the package
python_rich = { path = "./python_rich" }
# if the directory contains a `recipe.yaml`, `rattler-build` will be used.
python_rich_by_file = { path = "./recipe_by_file/recipe.yaml" }

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
rich = ">=13.9.4,<14"

```

## CLI Commands

Using the preview feature you can now build packages from source.

- `pixi build` has been added and will build a `.conda` file out of your package.
- Other commands like `pixi install` and `pixi run` automatically make use of the build feature when a `path`, `git` or `url` dependency is present.
