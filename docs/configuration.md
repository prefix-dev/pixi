---
part: pixi
title: Configuration
description: Learn what you can do in the pixi.toml configuration.
---

The `pixi.toml` is the pixi project configuration file, also known as the project manifest.

A `toml` file is structured in different tables.
This document will explain the usage of the different tables.
For more technical documentation check [crates.io](https://docs.rs/pixi/latest/pixi/project/manifest/struct.ProjectManifest.html).

## The `project` table
The minimally required information in the `project` table is:
```toml
[project]
name = "project-name"
authors = ["John Doe <j.doe@prefix.dev>"]
channels = ["conda-forge"]
platforms = ["linux-64"]
```

### `name`
The name of the project.
```toml
[project]
name = "project-name"
```

### `version` (optional)
The version of the project.
This should be a valid version based on the conda Version Spec.
See the [version documentation](https://docs.rs/rattler_conda_types/latest/rattler_conda_types/struct.Version.html), for an explanation of what is allowed in a Version Spec.
```toml
[project]
version = "1.2.3"
```

### `authors`
This is a list of authors of the project.
```toml
[project]
authors = ["John Doe <j.doe@prefix.dev>", "Marie Curie <mss1867@gmail.com>"]
```

### `channels`
This is a list that defines the channels used to fetch the packages from.
If you want to use channels hosted on `anaconda.org` you only need to use the name of the channel directly.
```toml
[project]
channels = ["conda-forge", "robostack", "bioconda", "nvidia", "pytorch"]
```

Channels situated on the file system are also supported with **absolute** file paths:
```toml
[project]
channels = ["conda-forge", "file:///home/user/staged-recipes/build_artifacts"]
```

To access private or public channels on [prefix.dev](https://prefix.dev/channels) or [Quetz](https://github.com/mamba-org/quetz) use the url including the hostname:
```toml
[project]
channels = ["conda-forge", "https://repo.prefix.dev/channel-name"]
```

### `platforms`
Defines the list of platforms that the project supports.
Pixi solves the dependencies for all these platforms and puts them in the lockfile (`pixi.lock`).

```toml
[project]
platforms = ["win-64", "linux-64", "osx-64", "osx-arm64"]
```
The available platforms are listed here: [link](https://docs.rs/rattler_conda_types/latest/rattler_conda_types/enum.Platform.html)

### `description` (optional)
This should contain a short description of the project.
```toml
[project]
description = "A simple description"
```

### `license` (optional)
The license as a valid [SPDX](https://spdx.org/licenses/) string (e.g. MIT AND Apache-2.0)
```toml
[project]
license = "MIT"
```

### `license-file` (optional)
Relative path to the license file.
```toml
[project]
license-file = "LICENSE.md"
```

### `readme` (optional)
Relative path to the README file.
```toml
[project]
readme = "README.md"
```

### `homepage` (optional)
URL of the project homepage.
```toml
[project]
homepage = "https://pixi.sh"
```

### `repository` (optional)
URL of the project source repository.
```toml
[project]
repository = "https://github.com/prefix-dev/pixi"
```

### `documentation` (optional)
URL of the project documentation.
```toml
[project]
documentation = "https://pixi.sh"
```

## The `tasks` table
Tasks are a way to automate certain custom commands in your project.
For example, a `lint` or `format` step.
Tasks in a pixi project are essentially cross-platform shell commands, with a unified syntax across platforms.
For more in-depth information, check the [Advanced tasks documentation](advanced/advanced_tasks.md).
Pixi's tasks are run in a pixi environment using `pixi run` and are executed using the [`deno_task_shell`](advanced/advanced_tasks.md#our-task-runner-deno_task_shell).

```toml
[tasks]
simple = "echo This is a simple task"
cmd = { cmd="echo Same as a simple task but now more verbose"}
depending = { cmd="echo run after simple", depends_on="simple"}
alias = { depends_on=["depending"]}
```
You can modify this table using [`pixi task`](cli.md#task).
!!! note
    Specify different tasks for different platforms using the [target](#the-target-table) table


## The `system-requirements` table
The system requirements are used to define minimal system specifications used during dependency resolution.
For example, we can define a unix system with a specific minimal libc version.
This will be the minimal system specification for the project.
System specifications are directly related to the [virtual packages](https://conda.io/projects/conda/en/latest/user-guide/tasks/manage-virtual.html).

Currently, the specified **defaults** are the same as [conda-lock](https://github.com/conda/conda-lock)'s implementation:
<Tabs groupId="Platforms">
    <TabItem value="Linux">
        ```toml title="default system requirements"
        [system-requirements]
        unix = true
        linux = "5.10"
        libc = { family="glibc", version="2.17" }
        ```
    </TabItem>
    <TabItem value="Windows">
        ```toml title="default system requirements"
        [system-requirements]
        windows = true
        ```
    </TabItem>
    <TabItem value="Osx-64">
        ```toml title="default system requirements"
        [system-requirements]
        unix = true
        macos = "10.15"
        ```
    </TabItem>
    <TabItem value="Osx-arm64">
        ```toml title="default system requirements"
        [system-requirements]
        unix = true
        macos = "11.0"
        ```
    </TabItem>
</Tabs>

Only if a project requires a different set should you define them.

For example, when installing environments on old versions of linux.
You may encounter the following error:
```
× The current system has a mismatching virtual package. The project requires '__linux' to be at least version '5.10' but the system has version '4.12.14'
```
This suggests that the system requirements for the project should be lowered.
To fix this, add the following table to your configuration:
```
[system-requirements]
linux = "4.12.14"
```

## The `dependencies` table(s)
This section defines what dependencies you would like to use for your project.

There are multiple dependencies tables.
The default is `[dependencies]`, which are dependencies that are shared across platforms.

Dependencies are defined using a [VersionSpec](https://docs.rs/rattler_conda_types/latest/rattler_conda_types/version_spec/enum.VersionSpec.html).
A `VersionSpec` combines a [Version](https://docs.rs/rattler_conda_types/latest/rattler_conda_types/struct.Version.html) with an optional operator.

Some examples are:
```toml
# Use this exact package version
package0 = "1.2.3"
# Use 1.2.3 up to 1.3.0
package1 = "~=1.2.3"
# Use larger than 1.2 lower and equal to 1.4
package2 = ">1.2,<=1.4"
# Bigger or equal than 1.2.3 or lower not including 1.0.0
package3 = ">=1.2.3|<1.0.0"
```

Dependencies can also be defined as a mapping where it is using a [matchspec](https://docs.rs/rattler_conda_types/latest/rattler_conda_types/struct.NamelessMatchSpec.html):
```toml
package0 = { version = ">=1.2.3", channel="conda-forge" }
package1 = { version = ">=1.2.3", build="py34_0" }
```

!!! tip
    The dependencies can be easily added using the `pixi add` command line.
    Running `add` for an existing dependency will replace it with the newest it can use.

!!! note
    To specify different dependencies for different platforms use the [target](#the-target-table) table


### `dependencies`
Add any conda package dependency that you want to install into the environment.
Don't forget to add the channel to the project table should you use anything different than `conda-forge`.
Even if the dependency defines a channel that channel should be added to the `project.channels` list.

```toml
[dependencies]
python = ">3.9,<=3.11"
rust = "1.72"
pytoch-cpu = { version = "~=1.1", channel = "pytorch" }
```

### `pypi-dependencies` (Beta feature)
Add any PyPI package that you want to install in the environment after the conda installation is finished.
These are not available on [prefix.dev](https://prefix.dev/channels) but on [pypi.org](https://pypi.org/).
!!! warning "Important considerations"
    - **Stability**: PyPI packages might be less stable than their conda counterparts. Prefer using conda packages in the `dependencies` table where possible.
    - **Compatibility limitations**: Currently, pixi doesn't support:
        - `git` dependencies (`git+https://github.com/package-org/package.git`)
        - Source dependencies
        - Private PyPI repositories
    - **Version specification**: These dependencies don't follow the conda matchspec specification.
    The `version` is a [`VersionSpecifier`](https://docs.rs/pep440_rs/0.3.12/pep440_rs/struct.VersionSpecifiers.html) and the `extras` are a list of `Strings`.
    So see the example below to see what type of definition is allowed.


```toml
[dependencies]
python = ">=3.6" # Python is needed for the pypi dependencies!

[pypi-dependencies]
pytest = "*"  # This means any version (this `*` is custom in pixi)
pre-commit = "~=3.5.0" # Single string is of type VersionSpecifiers
requests = {version = ">= 2.8.1, ==2.8.*", extras=["security", "tests"]} # Using the map allows the user to add `extras`
```

??? info "We use `rip` not `pip`"
    We use [`rip`](https://github.com/prefix-dev/rip) which is our custom pypi package resolver.
    The `rip` resolve step is invoked after the conda dependencies have been resolved.
    As the conda packages can also install python packages, which are used in the rip resolver.
    Also `rip` needs to know the version of python that is being used.

### `host-dependencies`

This table contains dependencies that are needed to build your project but which should not be included when your project is installed as part of another project.
In other words, these dependencies are available during the build but are no longer available when your project is installed.
Dependencies listed in this table are installed for the architecture of the target machine.

```toml
[host-dependencies]
python = "~=3.10.3"
```

Typical examples of host dependencies are:

* Base interpreters: a Python package would list `python` here and an R package would list `mro-base` or `r-base`.
* Libraries your project links against during compilation like `openssl`, `rapidjson`, or `xtensor`.

### `build-dependencies`
This table contains dependencies that are needed to build the project.
Different from `dependencies` and `host-dependencies` these packages are installed for the architecture of the _build_ machine.
This enables cross-compiling from one machine architecture to another.

```toml
[build-depencencies]
cmake = "~=3.24"
```

Typical examples of build dependencies are:

* Compilers are invoked on the build machine, but they generate code for the target machine.
If the project is cross-compiled, the architecture of the build and target machine might differ.
* `cmake` is invoked on the build machine to generate additional code- or project-files which are then include in the compilation process.

!!! info
    The _build_ target refers to the machine that will execute the build.
    Programs and libraries installed by these dependencies will be executed on the build machine.

    For example, if you compile on a MacBook with an Apple Silicon chip but target Linux x86_64 then your *build* platform is `osx-arm64` and your *host* platform is `linux-64`.


## The `activation` table
If you want to run an activation script inside the environment when either doing a `pixi run` or `pixi shell` these can be defined here.
The scripts defined in this table will be sourced when the environment is activated using `pixi run` or `pixi shell`

```toml
[activation]
scripts = ["env_setup.sh"]
```
!!! note
    Specify different scripts for different platforms using the [target](#the-target-table) table



## The `target` table
The target table is a table that allows for platform specific configuration.
Allowing you to make different sets of tasks or dependencies per platform.

The target table is currently implemented for the following sub-tables:

- [`activation`](#the-activation-table)
- [`dependencies`](#dependencies)
- [`tasks`](#the-tasks-table)

The target table is defined using `[target.PLATFORM.SUB-TABLE]`.
E.g `[target.linux-64.dependencies]`
The platform can be any of the target [platforms](#platforms) but must also be defined there.
The sub-table can be any of the specified above.

To make it a bit more clear, let's look at an example below.
Currently, pixi combines the top level tables like `dependencies` with the target-specific ones into a single set.
Which, in the case of dependencies, can both add or overwrite dependencies.
In the example below, we have `cmake` being used for all targets but on `osx-64` a different version of python will be selected.
```toml
[dependencies]
cmake = "3.26.4"
python = "3.10"

[target.osx-64.dependencies]
python = "3.11"
```

Here are some more examples:
```toml
[target.win-64.activation]
scripts = ["setup.ps1"]

[target.win-64.dependencies]
msmpi = "~=10.1.1"

[target.win-64.build-dependecies]
vs2022_win-64 = "19.36.32532"

[target.win-64.tasks]
tmp = "echo $TEMP"

[target.osx-64.dependencies]
clang = ">=16.0.6"
```
