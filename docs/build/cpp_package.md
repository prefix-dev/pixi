# Building a C++ example

This example shows how to build a C++ project with CMake and use it together with `pixi-build`.
To read more about how building packages work with pixi see the [Getting Started](./getting_started.md) guide.

## Creating a new project

To get started, create a new project with pixi:

```bash
pixi init
```

This should give you the basic `pixi.toml` to get started.

## The `pixi.toml` file
Use the following `pixi.toml` file, you can hover over the annotations to see why each step was added.

```toml
--8<-- "docs/source_files/pixi_tomls/pixi_build_cpp/pixi.toml"
```

1. Add the **preview** feature `pixi-build` that enables pixi to build the package.
2. These are the workspace dependencies and we add a reference to our own package.
3. Let's add a task that will run our test, for this we require a python interpreter.
4. This is where we specify the package name and version.
   This section denotes that there is both a worskpace and a package defined by this `pixi.toml` file.
5. We use `pixi-build-cmake` as the build-system, so that we get the backend to build cmake packages.
6. We use the [nanobind](https://github.com/wjakob/nanobind) package to build our bindings.
7. We need python to build the bindings, so we add a host dependency on python.
8. We override the cmake version to ensure it matches our `CMakeLists.txt` file.

Next lets add the `CMakeList.txt` file:
```CMake
--8<-- "docs/source_files/pixi_tomls/pixi_build_cpp/CMakeLists.txt"
```
