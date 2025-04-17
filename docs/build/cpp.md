This example shows how to build a C++ package with CMake and use it together with `pixi-build`.
To read more about how building packages work with Pixi see the [Getting Started](./getting_started.md) guide.

We'll start off by creating a workspace that use [nanobind](https://github.com/wjakob/nanobind) to build Python bindings.
That we can also test using pixi.
We'll later combine this example together with a Python package.

!!! warning
    `pixi-build` is a preview feature, and will change until it is stabilized.
    Please keep that in mind when you use it for your workspaces.

## Creating a New Workspace

To get started, create a new workspace with pixi:

```bash
pixi init cpp_math
```

This should give you the basic `pixi.toml` to get started.

We'll now create the following source directory structure:
```bash
cpp_math/
├── CMakeLists.txt
├── pixi.toml
├── .gitignore
└── src
    └── math.cpp
```

## Creating the workspace files
Next up we'll create the:

- `pixi.toml` file that will be used to configure pixi.
- `CMakeLists.txt` file that will be used to build the bindings.
- `src/math.cpp` file that will contain the bindings.

### The `pixi.toml` file
Use the following `pixi.toml` file, you can hover over the annotations to see why each step was added.

```toml
--8<-- "docs/source_files/pixi_workspaces/pixi_build/cpp/pixi.toml"
```

1. Add the **preview** feature `pixi-build` that enables Pixi to build the package.
2. These are the workspace dependencies. We add our own package as well as Python so that we can later run our package.
3. Let's add a task that will run our test
4. This is where we specify the package name and version.
   This section denotes that there is both a workspace and a package defined by this `pixi.toml` file.
5. We use `pixi-build-cmake` as the build-system, so that we get the backend to build cmake packages.
6. We use the [nanobind](https://github.com/wjakob/nanobind) package to build our bindings.
7. We need python to build the bindings, so we add a host dependency on the `python` package.
8. We override the cmake version to ensure it matches our `CMakeLists.txt` file.
9. Optionally, we can add extra arguments to the CMake invocation (e.g. `-DCMAKE_BUILD_TYPE=Release` or `-DUSE_FOOBAR=True`). This totally depends on the specific workspace / CMakeLists.txt file.

### The `CMakeLists.txt` file

Next lets add the `CMakeList.txt` file:
```CMake
--8<-- "docs/source_files/pixi_workspaces/pixi_build/cpp/CMakeLists.txt"
```

1. Find `python`, this actually finds anything above 3.8, but we are using 3.8 as a minimum version.
2. Because we are using `python` in a conda environment we need to query the python interpreter to find the `nanobind` package.
3. Because we want to make the installation directory independent of the python version, we query the python `site-packages` directory.
4. Find the installed nanobind package.
5. Use our bindings file as the source file.
6. Install the bindings into the designated prefix.

### The `src/math.cpp` file

Next lets add the `src/math.cpp` file, this one is quite simple:

```cpp
--8<-- "docs/source_files/pixi_workspaces/pixi_build/cpp/src/math.cpp"
```

1. We define a function that will be used to add two numbers together.
2. We bind this function to the python module using the `nanobind` package.

## Testing if everything works
Now that we have created the files we can test if everything works:

```bash
$ pixi run start
3
```

This command builds the bindings, installs them and then runs the `test` task.

## Conclusion

In this tutorial, we created a Pixi package using C++.
It can be used as-is, to upload to a conda channel.
In another tutorial we will learn how to add multiple Pixi packages to the same workspace and let one Pixi package use another.

Thanks for reading! Happy Coding 🚀

Any questions? Feel free to reach out or share this tutorial on [X](https://twitter.com/prefix_dev), [join our Discord](https://discord.gg/kKV8ZxyzY4), [e-mail](mailto:hi@prefix.dev) us or follow our [GitHub](https://github.com/prefix-dev).
