# Tutorial: Building a C++ package

This example shows how to build a C++ project with CMake and use it together with `pixi-build`.
To read more about how building packages work with pixi see the [Getting Started](./getting_started.md) guide.

We'll start off by creating a project that use [nanobind](https://github.com/wjakob/nanobind) to build Python bindings.
That we can also test using pixi.
We'll later combine this example together with a Python package.

## Creating a new project

To get started, create a new project with pixi:

```bash
pixi init python_bindings
```

This should give you the basic `pixi.toml` to get started.

We'll now create the following source directory structure:
```bash
python_bindings/
├── CMakeLists.txt
├── pixi.toml
├── .gitignore
└── src
    └── bindings.cpp
```

## Creating the project files
Next up we'll create the:

- `pixi.toml` file that will be used to configure pixi.
- `CMakeLists.txt` file that will be used to build the bindings.
- `src/bindings.cpp` file that will contain the bindings.

### The `pixi.toml` file
Use the following `pixi.toml` file, you can hover over the annotations to see why each step was added.

```toml
--8<-- "docs/source_files/pixi_projects/pixi_build_cpp/pixi.toml"
```

1. Add the **preview** feature `pixi-build` that enables pixi to build the package.
2. These are the workspace dependencies, and we add a reference to our own package.
3. Let's add a task that will run our test
4. This is where we specify the package name and version.
   This section denotes that there is both a workspace and a package defined by this `pixi.toml` file.
5. We use `pixi-build-cmake` as the build-system, so that we get the backend to build cmake packages.
6. We use the [nanobind](https://github.com/wjakob/nanobind) package to build our bindings.
7. We need python to build the bindings, so we add a host dependency on the `python_abi` package.
8. We override the cmake version to ensure it matches our `CMakeLists.txt` file.
9. In order to use the package, users will need Python available. Let's add it to the `run-dependencies`.

### The `CMakeLists.txt` file

Next lets add the `CMakeList.txt` file:
```CMake
--8<-- "docs/source_files/pixi_projects/pixi_build_cpp/CMakeLists.txt"
```

1. Find `python`, this actually finds anything above 3.8, but we are using 3.8 as a minimum version.
2. Because we are using `python` in a conda environment we need to query the python interpreter to find the `nanobind` package.
3. Because we want to make the installation directory independent of the python version, we query the python `site-packages` directory.
4. Find the installed nanobind package.
5. Use our bindings file as the source file.
6. Install the bindings into the designated prefix.

### The `src/bindings.cpp` file

Next lets add the `src/bindings.cpp` file, this one is quite simple:

```cpp
--8<-- "docs/source_files/pixi_projects/pixi_build_cpp/src/bindings.cpp"
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

In this tutorial, we created a pixi package using C++.
It can be used as-is, to upload to a conda channel.
In another tutorial we will learn how to add multiple pixi packages to the same workspace and let one pixi package use another.

Thanks for reading! Happy Coding 🚀

Any questions? Feel free to reach out or share this tutorial on [X](https://twitter.com/prefix_dev), [join our Discord](https://discord.gg/kKV8ZxyzY4), [e-mail](mailto:hi@prefix.dev) us or follow our [GitHub](https://github.com/prefix-dev).
