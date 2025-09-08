This example shows how to build a C++ package with CMake and use it together with `pixi-build`. To read more about how building packages work with Pixi see the [Getting Started](../getting_started/) guide. You might also want to check out the [documentation](https://prefix-dev.github.io/pixi-build-backends/backends/pixi-build-cmake/) for the `pixi-build-cmake` backend.

We'll start off by creating a workspace that use [nanobind](https://github.com/wjakob/nanobind) to build Python bindings. That we can also test using pixi. We'll later combine this example together with a Python package.

Warning

`pixi-build` is a preview feature, and will change until it is stabilized. Please keep that in mind when you use it for your workspaces.

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
[workspace]
channels = ["https://prefix.dev/conda-forge"]
platforms = ["osx-arm64", "linux-64", "osx-64", "win-64"]
preview = ["pixi-build"]                                  # (1)!
[dependencies] # (2)!
cpp_math = { path = "." }
python = "*"
[tasks]
start = "python -c 'import cpp_math as b; print(b.add(1, 2))'" # (3)!
[package] # (4)!
name = "cpp_math"
version = "0.1.0"
[package.build]
backend = { name = "pixi-build-cmake", version = "0.3.*" }
[package.build.config]
extra-args = ["-DCMAKE_BUILD_TYPE=Release"] # (9)!
[package.host-dependencies]
cmake = "3.20.*"   # (8)!
nanobind = "2.4.*" # (6)!
python = "3.12.*"  # (7)!

```

1. Add the **preview** feature `pixi-build` that enables Pixi to build the package.
1. These are the workspace dependencies. We add our own package as well as Python so that we can later run our package.
1. Let's add a task that will run our test
1. This is where we specify the package name and version. This section denotes that there is both a workspace and a package defined by this `pixi.toml` file.
1. We use `pixi-build-cmake` as the build-system, so that we get the backend to build cmake packages.
1. We use the [nanobind](https://github.com/wjakob/nanobind) package to build our bindings.
1. We need python to build the bindings, so we add a host dependency on the `python` package.
1. We override the cmake version to ensure it matches our `CMakeLists.txt` file.
1. Optionally, we can add extra arguments to the CMake invocation (e.g. `-DCMAKE_BUILD_TYPE=Release` or `-DUSE_FOOBAR=True`). This totally depends on the specific workspace / CMakeLists.txt file.

### The `CMakeLists.txt` file

Next lets add the `CMakeList.txt` file:

```CMake
cmake_minimum_required(VERSION 3.20...3.27)
project(cpp_math)
find_package(Python 3.8 COMPONENTS Interpreter Development.Module REQUIRED) # (1)!
execute_process(
  COMMAND "${Python_EXECUTABLE}" -m nanobind --cmake_dir
  OUTPUT_STRIP_TRAILING_WHITESPACE OUTPUT_VARIABLE nanobind_ROOT
) # (2)!
execute_process(
    COMMAND ${Python_EXECUTABLE} -c "import sysconfig; print(sysconfig.get_path('purelib'))"
    OUTPUT_VARIABLE PYTHON_SITE_PACKAGES
    OUTPUT_STRIP_TRAILING_WHITESPACE
) # (3)!
find_package(nanobind CONFIG REQUIRED) # (4)!
nanobind_add_module(${PROJECT_NAME} src/math.cpp) # (5)!
install( # (6)!
    TARGETS ${PROJECT_NAME}
    EXPORT ${PROJECT_NAME}Targets
    LIBRARY DESTINATION ${PYTHON_SITE_PACKAGES}
    ARCHIVE DESTINATION ${CMAKE_INSTALL_LIBDIR}
    RUNTIME DESTINATION ${BINDIR}
)

```

1. Find `python`, this actually finds anything above 3.8, but we are using 3.8 as a minimum version.
1. Because we are using `python` in a conda environment we need to query the python interpreter to find the `nanobind` package.
1. Because we want to make the installation directory independent of the python version, we query the python `site-packages` directory.
1. Find the installed nanobind package.
1. Use our bindings file as the source file.
1. Install the bindings into the designated prefix.

### The `src/math.cpp` file

Next lets add the `src/math.cpp` file, this one is quite simple:

```cpp
#include <nanobind/nanobind.h>
int add(int a, int b) { return a + b; } // (1)!
NB_MODULE(cpp_math, m)
{
    m.def("add", &add); // (2)!
}

```

1. We define a function that will be used to add two numbers together.
1. We bind this function to the python module using the `nanobind` package.

## Testing if everything works

Now that we have created the files we can test if everything works:

```bash
$ pixi run start
3

```

This command builds the bindings, installs them and then runs the `test` task.

## Conclusion

In this tutorial, we created a Pixi package using C++. It can be used as-is, to upload to a conda channel. In another tutorial we will learn how to add multiple Pixi packages to the same workspace and let one Pixi package use another.

Thanks for reading! Happy Coding 🚀

Any questions? Feel free to reach out or share this tutorial on [X](https://twitter.com/prefix_dev), [join our Discord](https://discord.gg/kKV8ZxyzY4), [e-mail](mailto:hi@prefix.dev) us or follow our [GitHub](https://github.com/prefix-dev).
