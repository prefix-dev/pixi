
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


Since the build feature is still in preview, you have to add "pixi-build" to `workspace.preview`.

```toml
--8<-- "docs/source_files/pixi_tomls/simple_pixi_build.toml:preview"
```

In `package` you specify properties specific to the package you want to build.

```toml
--8<-- "docs/source_files/pixi_tomls/simple_pixi_build.toml:package"
```

Packages are built by using build backends.
By specifying `build-system.build-backend` and `build-system.channels` you determine which backend is used and from which channel it will be downloaded.
In this example, we are using `pixi-build-python` in order to build a Python package.

```toml
--8<-- "docs/source_files/pixi_tomls/simple_pixi_build.toml:build-system"
```

If the package itself has dependencies they need to be mentioned here.
The different kinds of dependencies are explained at the [dependency types chapter](dependency_types.md).

`simple_python` uses `hatchling` as Python build backend so this needs to be mentioned in `host-dependencies`

??? "Pixi backends and Python backends"
    Note, that these are two different kind of backends.
    Pixi backends can describe how build a conda package.
    One of these backends is called `pixi-build-python`, which allows building a Python package.
    Python backends like `hatchling` know how to build a Python package.
    So `hatchling` creates a Python package, and `pixi-build-python` turns the Python package into a conda package.


## Migrate to the new build feature

!!! note
    The new build feature is currently in preview, and both the manifest configuration and the build backends are subject to change.

To enable the new build feature, you need to add the correct build configuration to your `pixi.toml` file.
These instructions assume that the `pixi.toml` has been used by a `pixi` version `<0.39`.
Below, an example will be given for a pixi **project** containing a single python **package**.

1.  Enable the `build` feature in your `pixi.toml` file. And add the `[build-section]` to your `pixi.toml` file.
    For clarity, rename the `[project]` section to `[workspace]` and add the `preview` key.
    ```toml
    [workspace] # Used to be `project`
    # ... other project/workspace configuration
    preview = ["build"]
    ```

2. Add the `package` and the `build-system` section to your `pixi.toml` file.
    ```toml
    # This section marks the project as a pixi package.
    #
    # Normally a number of fields would be set here, like the name, version, etc.
    # However, since all these fields are already defined in the [project] section
    # at the top of this file they are not required.
    [package]

    # The build-system section defines the build system that will be used to turn
    # the source code of this package into a conda package. Similarly to the above
    # [build-system] section this section instructs pixi which build backend to
    # use. The build-backend is an executable that is installed and invoked by
    # pixi with the sole purpose to build the package.
    [build-system]
    # The name of the build backend to use. This name refers both to the name of
    # the package that provides the build backend and the name of the executable
    # inside the package that is invoked.
    #
    # The `build-backend` key also functions as a dependency declaration. At least
    # a version specifier must be added.
    build-backend = { name = "pixi-build-python", version = "*" }
    # These are the conda channels that are used to resolve the dependencies of the
    # build backend package.
    channels = [
      "https://prefix.dev/pixi-build-backends",
      "https://prefix.dev/conda-forge",
    ]
    ```
3. ??? note "Host dependencies"
       Read up on host-dependencies in the [Dependency Types](./dependency_types.md#host-dependencies).
   Add the correct *host dependencies* to your `pixi.toml` file.
   We need to add these to the host dependencies, because of it using the wrong python prefix otherwise.
   We want to change this in the future, to be a bit less of a hassle.
    ```toml
    [host-dependencies]
    # To be able to install this pyproject we need to install the dependencies of
    # the python build-system defined above. Note that different from the
    # pyproject build-system this refers to a conda package instead of a pypi
    # package.
    hatchling = "==1.26.3"
    # This way uv is used instead of pip for building
    uv = "*"
    ```
4. Add a reference to your own source.
    ```toml
    [dependencies]
    name_of_pkg = { path = "." }
    ```

Now you can build your package with pixi:
  * `pixi build` will build your source package into a `.conda` file.
  * `pixi install` will install your source package into a conda environment.
