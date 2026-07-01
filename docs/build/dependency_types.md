If you add a package to the [dependency table](../reference/pixi_manifest.md#dependencies) of a feature
that dependency will be available in all environments that include that feature.
The dependencies of a package that is being built are a bit more granular.
Here you can see the three types of dependencies for a simple C++ package.

```toml
--8<-- "docs/source_files/pixi_tomls/dependency_types.toml:dependencies"
```

Each dependency is used at a different step of the package building process.
`cxx-compiler` is used to build the package, `catch` will be linked into the package and `git` will be available during runtime.

Let's delve deeper into the various types of package dependencies and their specific roles in the build process.

!!! note "pixi-build-rattler-build"
    The `pixi-build-rattler-build` backend only regards dependencies defined in the `recipe.yaml` 

### [Build Dependencies](../reference/pixi_manifest.md#build-dependencies)
!!! note "pixi-build-cmake"
    When using the `pixi-build-cmake` backend you do not need to specify `cmake` or the compiler as a dependency.
    The backend will install `cmake`, `ninja` and the C++ compilers by default.

This table contains dependencies that are needed to build the workspace.
Different from dependencies and host-dependencies these packages are installed for the architecture of the build machine.
This enables cross-compiling from one machine architecture to another.

Typical examples of build dependencies are:

- Compilers are invoked on the build machine, but they generate code for the target machine.
  If the package is cross-compiled, the architecture of the build and target machine might differ.
- `cmake` is invoked on the build machine to generate additional files which are then include in the compilation process.

!!! info
    The _build_ target refers to the machine that will execute the build.
    Programs and libraries installed by these dependencies will be executed on the build machine.

    For example, if you compile on a MacBook with an Apple Silicon chip but target Linux x86_64 then your *build* platform is `osx-arm64` and your *host* platform is `linux-64`.

### [Host Dependencies](../reference/pixi_manifest.md#host-dependencies)

Host dependencies are the dependencies needed during build/link time that are specific to the host machine.
The difference to build dependencies becomes for example important during cross compilation.
The compiler is a build dependency since it is specific to your machine.
In contrast, the libraries you link to are host dependencies since they are specific to the host machine.
Typical examples of host dependencies are:

- Base interpreters: a Python package would list `python` here and an R package would list `mro-base` or `r-base`.
- Libraries your package links against like `openssl`, `rapidjson`, or `xtensor`.

#### Python Code
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

#### Native Code
When cross-compiling, you might need to specify host dependencies that should have the *target* machine architecture, and are used during the build process.
When linking a library, for example.
Let's recap an explanation that can be found here [A Master Guide To Linux Cross-Compiling](https://ruvi-d.medium.com/a-master-guide-to-linux-cross-compiling-b894bf909386)

- *Build machine*: where the code is built.
- *Host machine*: where the built code runs.
- *Target machine*: where the binaries spit out by the built code runs.

Let's say we are using a Linux PC (linux-64) to cross compile a CMake application called `Awesome` to run on a Linux ARM target machine (linux-aarch64).
We would get the following table:

| Component |    Type     | Build  |  Host  | Target |
|-----------|-------------|--------|--------|--------|
| GCC       | Compiler    | x86_64 | x86_64 | aarch64|
| CMake     | Build tool  | x86_64 | x86_64 | N/A    |
| Awesome   | Application | x86_64 | aarch64  | N/A  |

So if I need to use a library like SDL2, I would need to add it to the `host-dependencies` table.
As the machine running `Awesome` will have a different host architecture than the build architecture.

Giving you something like this in your manifest file:

```toml
 # in our example these dependencies will use the aarch64 binaries
[host-dependencies]
sdl2 = "*"
```

#### Run-Exports

Conda packages, can define `run-exports`, that are dependencies that when specified in the `host-dependencies` section, will be implicitly be added to the `run-dependencies` section.
This is useful to avoid having to specify the same dependencies in both sections.
As most packages on conda-forge will have these `run-exports` defined.
When using something like `zlib`, you would only need to specify it in the `host-dependencies` section, and it will be used as a run-dependency automatically.


### [Dependencies (Run Dependencies)](../reference/pixi_manifest.md#dependencies)

These are the dependencies that are required to when running the package, they are the most common dependencies.
And are what you would usually use in a `workspace`.

### [Extra Dependencies](../reference/pixi_manifest.md#extra-dependencies)

Package manifests can define groups of *extra dependencies* in `package.extra-dependencies`.

Each group is a named set of additional run dependencies that can be enabled when needed. Dependencies use the same conda package specification syntax as `run-dependencies`.

This follows the conda [optional dependencies CEP](https://github.com/conda/ceps/blob/main/cep-0044.md).

For example, here is a complete package manifest that declares a `test` group for its test suite and a `cuda` group for GPU support:

```toml
[package]
name = "mypackage"
version = "0.1.0"

[package.build]
backend = { name = "pixi-build-python", version = "0.*" }

[package.run-dependencies]
numpy = ">=2"

[package.extra-dependencies.test]
pytest = ">=8"
hypothesis = "*"

[package.extra-dependencies.cuda]
cupy = ">=13"
```

A workspace that depends on `mypackage` as a source dependency can enable one or more of these groups through the `extras` field:

```toml
[workspace]
channels = ["conda-forge"]
platforms = ["linux-64"]

[dependencies]
# Pulls in the `cuda` group of `mypackage` in addition to its regular run dependencies.
mypackage = { path = "./mypackage", extras = ["cuda"] }
```

### [Run Constraints](../reference/pixi_manifest.md#run-constraints)

Constraints that apply to the package's run environment, but only when the constrained package is pulled in as a dependency by something else.
They never cause a package to be installed on their own. To do that, use run-dependencies (#dependencies-run-dependencies).

This corresponds to conda's `run_constrained` package metadata.

## Conditional Dependencies

Any of the dependency tables above can hold dependencies that only apply when a condition holds.
Write the condition as an `if(<expression>)` key inside the dependency table:

```toml
# Only needed when cross-compiling (host platform differs from build platform).
[package.build-dependencies."if(host_platform != build_platform)"]
cross-python = "*"

# Only on Linux.
[package.host-dependencies."if(host_platform == 'linux-64')"]
libgl-devel = ">=1.7.0,<2"

# Based on a build variant.
[package.host-dependencies."if(matches(python, '>=3.10'))"]
exceptiongroup = "*"
```

The expression is passed through verbatim to the build-backend.
At the time of this writing all build backends are backed by [rattler-build](https://rattler.build), so any selector it understands works, including the boolean operators `and`, `or` and `not`.
Three platform variables are available:

- `build_platform`: the platform the build runs on.
- `host_platform`: the platform the package is built for.
  Differs from `build_platform` when cross-compiling.
- `target_platform`: the run platform.
  Differs from `host_platform` for `noarch` packages.

The platform families `unix`, `linux`, `win` and `osx` are also available as bare booleans, e.g. `if(unix)`.

!!! note
    `if(...)` conditions are only available in the `[package]` dependency tables.
    The workspace `[target.*]` tables continue to accept platform names only.

## Inheriting Versions From the Workspace

When several packages in the same workspace share dependency versions you can
declare them once in `[workspace.dependencies]` and inherit per entry from
every member's package tables (and from `[package.build.backend]`).
See [Workspace Dependencies](workspace_dependencies.md) for the full rules
around overrides and errors.
