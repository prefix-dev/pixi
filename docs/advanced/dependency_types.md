# Run, Host and Build Dependencies

If you add package to the [dependency](../reference/pixi_manifest.md#dependencies) table that package will be available in your pixi environment.
As soon as you are using the [build feature](../preview_features/build.md) to build a package, it is important to know how the other dependency types work.
This document describes the different types of dependencies that can be used in the manifest and what the differences are.

Currently, there are 3 types of dependencies, with the following manifest specification:

1. Build dependencies: `build-dependencies`
2. Host dependencies: `host-dependencies`
3. Runtime dependencies: `dependencies` or `run-dependencies`

The other dependency type [`pypi-dependencies`](../reference/pixi_manifest.md#pypi-dependencies) is not covered in this document, as it is not used by the conda ecosystem.

Here we have the dependencies of a simple C++ package
```toml
--8<-- "docs/source_files/pixi_tomls/dependency_types.toml:dependencies"
```

<!-- TODO: Let's use this example to explain the different dependency types -->


### [Build Dependencies](../reference/pixi_manifest.md#build-dependencies)
??? note "pixi-build-cmake"
    When using the `pixi-build-cmake` backend you do not need to specify `cmake` or the compiler as a dependency.
    The backend will install `cmake`, `ninja` and the C++ compilers by default.

This table contains dependencies that are needed to build the project.
Different from dependencies and host-dependencies these packages are installed for the architecture of the build machine.
This enables cross-compiling from one machine architecture to another.

Typical examples of build dependencies are:

- Compilers are invoked on the build machine, but they generate code for the target machine.
  If the project is cross-compiled, the architecture of the build and target machine might differ.
- `cmake` is invoked on the build machine to generate additional code- or project-files which are then include in the compilation process.

!!! info
    The _build_ target refers to the machine that will execute the build.
    Programs and libraries installed by these dependencies will be executed on the build machine.

    For example, if you compile on a MacBook with an Apple Silicon chip but target Linux x86_64 then your *build* platform is `osx-arm64` and your *host* platform is `linux-64`.

### [Host Dependencies](../reference/pixi_manifest.md#host-dependencies)

<!-- TODO: Let's add one sentence what host dependencies are and how pixi treats them differently from run or build -->

Typical examples of host dependencies are:

- Base interpreters: a Python package would list `python` here and an R package would list `mro-base` or `r-base`.
- Libraries your project links against during compilation like `openssl`, `rapidjson`, or `xtensor`.

#### Python code
Because of the way building currently works, dependencies like `hatchling`,`pip`,`uv` etc. are host dependencies.
Otherwise, it would use the wrong python prefix during the build process.

This is more of a technical limitation, and we are looking into ways to make this less of a hassle.
But for now, you will need to add these dependencies to the `host-dependencies` section.

So as an example, say we want to use `hatchling` and `uv` as to build a python package.
You need to use, something like this in your manifest file:

```toml
[host-dependencies]
hatchling = "*"
uv = "*"
```

#### Native code
When cross-compiling, you might need to specify host dependencies that should have the *target* machine architecture, and are used during the build process.
For example, for linking a library.
Let's recap an explanation from here [A Master Guide To Linux Cross-Compiling](https://ruvi-d.medium.com/a-master-guide-to-linux-cross-compiling-b894bf909386)

- *Build machine*: where the code is built.
- *Host machine*: where the built code runs.
- *Target machine*: where the binaries spit out by the built code runs.

For example, Lets say we are using a Linux PC (x86_64-linux-gnu) to cross compile a CMake application called `Awesome` to run on a Linux ARM target machine (armv7-linux-gnueabihf).
We would get the following table:

| Component |    Type     | Build  |  Host  | Target |
|-----------|-------------|--------|--------|--------|
| GCC       | Compiler    | x86_64 | x86_64 | armv7  |
| CMake     | Build tool  | x86_64 | x86_64 | N/A    |
| Awesome   | Application | x86_64 | armv7  | N/A    |

So if I need to use a library like SDL2, I would need to add it to the `host-dependencies` table.
As `Awesome` has a different host architecture than the build architecture.

Giving you something like this in your manifest file:

```toml
[host-dependencies]
sdl2 = "*"
```

#### Run-exports

Conda packages, can define `run-exports`, that are dependencies that when specified in the `host-dependencies` section, will be implicitly be added to the `run-dependencies` section.
This is useful to avoid having to specify the same dependencies in both sections.
As most packages on conda-forge will have these `run-exports` defined.
When using something like `zlib`, you would only need to specify it in the `host-dependencies` section, and it will be used as a run-dependency automatically.


### [Dependencies (Run Dependencies)](../reference/pixi_manifest.md#dependencies)

These are the dependencies that are required to run the package, they are the most common dependencies.
And are what you would usually use in `project` or `workspace`.
