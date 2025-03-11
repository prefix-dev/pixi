# Tutorial: Building a C++ Package with rattler-build and recipe.yaml

In this tutorial, we will show you how to build the same C++ package as from [Building a C++ Package](cpp.md) tutorial using a `recipe.yaml` with `rattler-build`.

This approach may be useful when no build backend for your language or build system exists. You may use it for example when building a package that would require a `go` backend.


Another reason to use it when you would like to have more control over the build process, by passing custom flags to the build system, pre or post-process the build artifacts, which is not possible with the existing backends.


To illustrate this, we will use the same C++ package as in the previous tutorial, but this time we will use `rattler-build` to build it. This will unvail the hidden complexity of the build process, and give you a better grasp of how backends work.


!!! warning
    `pixi-build` is a preview feature, and will change until it is stabilized.
    Please keep that in mind when you use it for your workspaces.

!!! hint
    Prefer using a backend if it exists. This will give you a more streamlined and unified build experience.

## Creating a new workspace

To get started, create a new workspace with Pixi:

```bash
pixi init python_bindings
```

This should give you the basic `pixi.toml` to get started.

We'll now create the following source directory structure:
```bash
python_bindings/
├── CMakeLists.txt
├── pixi.toml
├── recipe.yaml
├── .gitignore
└── src
    └── bindings.cpp
```

## Creating the workspace files
Next up we'll create the:

- `pixi.toml` file that will be used to configure pixi.
- `CMakeLists.txt` file that will be used to build the bindings.
- `src/bindings.cpp` file that will contain the bindings.
- `recipe.yaml` file that will be used to build the package.

### The `pixi.toml` file
Use the following `pixi.toml` file, you can hover over the annotations to see why each step was added.

```toml
--8<-- "docs/source_files/pixi_workspaces/pixi_build/advanced_cpp/pixi.toml"
```

1. Add the **preview** feature `pixi-build` that enables Pixi to build the package.
2. These are the workspace dependencies. We add our own package `python_bindings` as well as Python so that we can later run our package.
3. Let's add a task that will run our test
4. This is where we specify the package name and version.
   This section denotes that there is both a workspace and a package defined by this `pixi.toml` file.
5. We use `pixi-build-rattler-build` as the build-system, so that we get the `rattler-build` to build the `recipe.yaml` present alongside the `pixi.toml`.
6. We use the [nanobind](https://github.com/wjakob/nanobind) package to build our bindings.
7. We need python to build the bindings, so we add a host dependency on the `python` package.


### The `CMakeLists.txt` file

Next lets add the `CMakeList.txt` file:
```CMake
--8<-- "docs/source_files/pixi_workspaces/pixi_build/advanced_cpp/CMakeLists.txt"
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
--8<-- "docs/source_files/pixi_workspaces/pixi_build/advanced_cpp/src/bindings.cpp"
```

1. We define a function that will be used to add two numbers together.
2. We bind this function to the python module using the `nanobind` package.


## The `recipe.yaml` file

Next lets add the `recipe.yaml` file that will be used to build the package:

```yaml
--8<-- "docs/source_files/pixi_workspaces/pixi_build/advanced_cpp/recipe.yaml"
```

1. Because we are specifying current directory as the source directory, `rattler-build` may skip files that are not tracked by git. We need to ignore `gitignore`.  If your files are already tracked by git, you can remove this line.
2. This build script configures and builds a `CMake` project using the `Ninja` build system. It sets various options such as the build type to `Release`, the installation prefix to `$PREFIX`, and enables shared libraries and compile commands export. The script then builds the project in the specified build directory `($SRC_DIR/../build)` and installs the built files to the installation directory.
3. For build dependencies we need compilers and the build systems `cmake` and `ninja`. Make sure that `cmake` version matches the one from `CMakeLists.txt` file.
4. For host dependencies we will need `python` and `nanobind`.

## Testing if everything works
Now that we have created the files we can test if everything works:

```bash
$ pixi run start
3
```

This command builds the bindings, installs them and then runs the `test` task.

## Conclusion

In this tutorial, we created a Pixi package using `rattler-build` and a `recipe.yaml` file. Using this approach, you have more control over the build process. For example, you could using `Make` instead of `Ninja` or you could use `make -j$(nproc)` to specify the number of jobs to run in parallel when building the package.

At the same time, you are losing all the benefit of heavy lifting that is done by language build backend.

Thanks for reading! Happy Coding 🚀

Any questions? Feel free to reach out or share this tutorial on [X](https://twitter.com/prefix_dev), [join our Discord](https://discord.gg/kKV8ZxyzY4), [e-mail](mailto:hi@prefix.dev) us or follow our [GitHub](https://github.com/prefix-dev).
