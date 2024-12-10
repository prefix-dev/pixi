!!! note
    This feature is currently in preview phase. To enable use the [preview feature](../reference/pixi_manifest.md#preview-features).
    ```toml
    [project]
    # .. other configuration
    preview = ["build"]
    ```

Sometimes you might need to build packages from source. This can be due to a variety of reasons, such as:

1. You are using the package for local development, and want to install and build the package into your workspace.
2. You want to use source for a package that is not available on conda-forge.
3. You want to build your package locally into a conda package.

We've been working to support these use-cases with the `build` feature in pixi. 
The vision is to enable building of packages from source, for any language, on any platform.

## High-level overview
There are a couple of key concepts that make it easier to understand how the `build` feature works. The two most important
additions are the concept of a *package* and a *build-system*.

### Package

The package defines the `build-system` and other fields in the future that are used to build your project.
Currently, the dependencies, host-dependencies and other dependency fields are associated with the package.
When you want to build a project you need to add this section.

### Build-system
This defines the backend that is used to build the package. 
The backend is an executable that is installed and invoked by pixi with the sole purpose to build the package.
Backends can be versioned and are installed from a conda channel, by pixi.
The currently available backends can be viewed in the [pixi-build-backends](https://prefix.dev/pixi-build-backends) channel.
The source of the backends is available in the [pixi-build-backends](https://github.com/prefix-dev/pixi-build-backends) repository.


## Migrate to the new build feature

!!! note
    The new build feature is currently in preview, and both the manifest configuration and the build backends are subject to change.

To enable the new build feature, you need to add the correct build configuration to your `pixi.toml` file.
Below, an example will be given for a pixi **project** containing a single python **package**. 

1.  Enable the `build` feature in your `pixi.toml` file. And add the `[build-section]` to your `pixi.toml` file.
    ```toml
    [project]
    # ... other configuration
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
3. Add the correct *host dependencies* to your `pixi.toml` file.
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
