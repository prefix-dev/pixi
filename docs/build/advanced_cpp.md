# Tutorial: Building a C++ Package with rattler-build and recipe.yaml

In this tutorial, we will show you how to build the same C++ package as from [Building a C++ Package](cpp.md) tutorial using a `recipe.yaml` with `rattler-build`.

This approach may be useful when no build backend for your language or build system exists.


Another reason to use it when you would like to have more control over the build process. At the time of writing this tutorial, `pixi-build-cmake` don't have profiles support, so we always build in `Release` mode. You could want to build your package in `Debug` mode, or other [build types](https://cmake.org/cmake/help/latest/variable/CMAKE_BUILD_TYPE.html#variable:CMAKE_BUILD_TYPE).
When using `recipe.yaml` you can customize the build process.


To illustrate this, we will use the same C++ package as in the previous tutorial, but this time we will use `rattler-build` to build it. This will unvail the hidden complexity of the build process, and give you a better grasp of how backends work.


!!! note
    In this tutorial we assume the knowledge from the [Building a C++ Package](cpp.md) tutorial. If you haven't read it yet, we recommend you to do so before continuing.
    The project structure and the source code will be the same as in the previous tutorial, so we may skip explicit explanations of some parts.

!!! warning
    `pixi-build` is a preview feature, and will change until it is stabilized.
    Please keep that in mind when you use it for your workspaces.

!!! hint
    Prefer using a backend if it exists. This will give you a more streamlined and unified build experience.

## Workspace structure

To get started, please recreate the structure of the workspace from the previous tutorial [Building a C++ Package](cpp.md).


### The `pixi.toml` file
Use the following `pixi.toml` file, you can hover over the annotations to see why each step was added.

```toml
--8<-- "docs/source_files/pixi_workspaces/pixi_build/advanced_cpp/pixi.toml"
```

1. Add the **preview** feature `pixi-build` that enables Pixi to build the package.
2. The main difference is that now we use `pixi-build-rattler-build` as the build-system. This means that `rattler-build` will be invoked to build the `recipe.yaml` present alongside the `pixi.toml`.


## The `recipe.yaml` file

Next lets add the `recipe.yaml` file that will be used to build the package:

```yaml
--8<-- "docs/source_files/pixi_workspaces/pixi_build/advanced_cpp/recipe.yaml"
```

1. Because we are specifying current directory as the source directory, `rattler-build` may skip files that are not tracked by git. If your files are already tracked by git, you can remove this configuration.
2. This build script configures and builds a `CMake` project using the `Ninja` build system. It sets various options such as the build type to `Release`, the installation prefix to `$PREFIX`, and enables shared libraries and compile commands export. The script then builds the project in the specified build directory `($SRC_DIR/../build)` and installs the built files to the installation directory.
3. For build dependencies we need compilers and the build systems `cmake` and `ninja`. Make sure that `cmake` version matches the one from `CMakeLists.txt` file.
4. For `python` bindings, we need `nanobind` and `python` itself. They are set in `host` dependencies section as we link them to the `python` where the bindings will be installed, not built.

## Testing if everything works
Now that we have created the files we can test if everything works using previously defined `pixi` task:
```toml
[tasks]
start = "python -c 'import python_bindings as b; print(b.add(1, 2))'"
```


```bash
$ pixi run start
3
```

This command builds the bindings, installs them and then runs the `test` task.

## Conclusion

In this tutorial, we created a Pixi package using `rattler-build` and a `recipe.yaml` file. Using this approach, we had more control over the build process. For example, we could changed the build type to `Debug` using `CMAKE_BUILD_TYPE`, use `Make` instead of `Ninja` by removing the `-GNinja` configuration or we could use `make -j$(nproc)` to specify the number of jobs to run in parallel when building the package.

At the same time, we lost all the benefit of heavy lifting that is done by language build backend.

!!! note
    At the time of writing this tutorial, `pixi-build-cmake` backend doesn't support modifying the existing arguments, only addition of new ones.

Thanks for reading! Happy Coding 🚀

Any questions? Feel free to reach out or share this tutorial on [X](https://twitter.com/prefix_dev), [join our Discord](https://discord.gg/kKV8ZxyzY4), [e-mail](mailto:hi@prefix.dev) us or follow our [GitHub](https://github.com/prefix-dev).
