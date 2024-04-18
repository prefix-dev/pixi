---
part: pixi
title: Configuration
description: Learn what you can do in the pixi.toml configuration.
---

The `pixi.toml` is the pixi project configuration file, also known as the project manifest.

A `toml` file is structured in different tables.
This document will explain the usage of the different tables.
For more technical documentation check pixi on [crates.io](https://docs.rs/pixi/latest/pixi/project/manifest/struct.ProjectManifest.html).

!!! tip
    We also support the `pyproject.toml` file. It has the same structure as the `pixi.toml` file. except that you need to prepend the tables with `tool.pixi` instead of just the table name.
    For example, the `[project]` table becomes `[tool.pixi.project]`.
    There are also some small extras that are available in the `pyproject.toml` file, checkout the [pyproject.toml](../advanced/pyproject_toml.md) documentation for more information.

## The `project` table

The minimally required information in the `project` table is:

```toml
[project]
name = "project-name"
channels = ["conda-forge"]
platforms = ["linux-64"]
```

### `name`

The name of the project.

```toml
name = "project-name"
```

### `channels`

This is a list that defines the channels used to fetch the packages from.
If you want to use channels hosted on `anaconda.org` you only need to use the name of the channel directly.

```toml
channels = ["conda-forge", "robostack", "bioconda", "nvidia", "pytorch"]
```

Channels situated on the file system are also supported with **absolute** file paths:

```toml
channels = ["conda-forge", "file:///home/user/staged-recipes/build_artifacts"]
```

To access private or public channels on [prefix.dev](https://prefix.dev/channels) or [Quetz](https://github.com/mamba-org/quetz) use the url including the hostname:

```toml
channels = ["conda-forge", "https://repo.prefix.dev/channel-name"]
```

### `platforms`

Defines the list of platforms that the project supports.
Pixi solves the dependencies for all these platforms and puts them in the lockfile (`pixi.lock`).

```toml
platforms = ["win-64", "linux-64", "osx-64", "osx-arm64"]
```

The available platforms are listed here: [link](https://docs.rs/rattler_conda_types/latest/rattler_conda_types/enum.Platform.html)

### `version` (optional)

The version of the project.
This should be a valid version based on the conda Version Spec.
See the [version documentation](https://docs.rs/rattler_conda_types/latest/rattler_conda_types/struct.Version.html), for an explanation of what is allowed in a Version Spec.

```toml
version = "1.2.3"
```

### `authors` (optional)

This is a list of authors of the project.

```toml
authors = ["John Doe <j.doe@prefix.dev>", "Marie Curie <mss1867@gmail.com>"]
```

### `description` (optional)

This should contain a short description of the project.

```toml
description = "A simple description"
```

### `license` (optional)

The license as a valid [SPDX](https://spdx.org/licenses/) string (e.g. MIT AND Apache-2.0)

```toml
license = "MIT"
```

### `license-file` (optional)

Relative path to the license file.

```toml
license-file = "LICENSE.md"
```

### `readme` (optional)

Relative path to the README file.

```toml
readme = "README.md"
```

### `homepage` (optional)

URL of the project homepage.

```toml
homepage = "https://pixi.sh"
```

### `repository` (optional)

URL of the project source repository.

```toml
repository = "https://github.com/prefix-dev/pixi"
```

### `documentation` (optional)

URL of the project documentation.

```toml
documentation = "https://pixi.sh"
```

### `conda-pypi-map` (optional)

Mapping of channel name or URL to location of mapping that can be URL/Path.
Mapping should be structured in `json` format where `conda_name`: `pypi_package_name`.
Example:

```json title="local/robostack_mapping.json"
{
  "jupyter-ros": "my-name-from-mapping",
  "boltons": "boltons-pypi"
}
```

If `conda-forge` is not present in `conda-pypi-map` `pixi` will use `prefix.dev` mapping for it.

```toml
conda-pypi-map = { "conda-forge" = "https://example.com/mapping", "https://repo.prefix.dev/robostack" = "local/robostack_mapping.json"}
```

## The `tasks` table

Tasks are a way to automate certain custom commands in your project.
For example, a `lint` or `format` step.
Tasks in a pixi project are essentially cross-platform shell commands, with a unified syntax across platforms.
For more in-depth information, check the [Advanced tasks documentation](../features/advanced_tasks.md).
Pixi's tasks are run in a pixi environment using `pixi run` and are executed using the [`deno_task_shell`](../features/advanced_tasks.md#our-task-runner-deno_task_shell).

```toml
[tasks]
simple = "echo This is a simple task"
cmd = { cmd="echo Same as a simple task but now more verbose"}
depending = { cmd="echo run after simple", depends_on="simple"}
alias = { depends_on=["depending"]}
download = { cmd="curl -o file.txt https://example.com/file.txt" , outputs=["file.txt"]}
build = { cmd="npm build", cwd="frontend", inputs=["frontend/package.json", "frontend/*.js"]}
run = { cmd="python run.py $ARGUMENT", env={ ARGUMENT="value" }}
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

=== "Linux"
    ```toml title="default system requirements for linux"
    [system-requirements]
    linux = "5.10"
    libc = { family="glibc", version="2.17" }
    ```

=== "Windows"
    ```toml title="default system requirements for windows"
    [system-requirements]
    ```

=== "Osx"
    ```toml title="default system requirements for osx"
    [system-requirements]
    macos = "10.15"
    ```

=== "Osx-arm64"
    ```toml title="default system requirements for osx-arm64"
    [system-requirements]
    macos = "11.0"
    ```

Only if a project requires a different set should you define them.

For example, when installing environments on old versions of linux.
You may encounter the following error:

```
× The current system has a mismatching virtual package. The project requires '__linux' to be at least version '5.10' but the system has version '4.12.14'
```

This suggests that the system requirements for the project should be lowered.
To fix this, add the following table to your configuration:

```toml
[system-requirements]
linux = "4.12.14"
```

#### Using Cuda in pixi

If you want to use `cuda` in your project you need to add the following to your `system-requirements` table:

```toml
[system-requirements]
cuda = "11" # or any other version of cuda you want to use
```

This informs the solver that cuda is going to be available, so it can lock it into the lockfile if needed.

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
pytorch-cpu = { version = "~=1.1", channel = "pytorch" }
```

### `pypi-dependencies`

??? info "Details regarding the PyPI integration"
    We use [`uv`](https://github.com/astral-sh/uv), which is a new fast pip replacement written in Rust.

    We integrate uv as a library, so we use the uv resolver, to which we pass the conda packages as 'locked'.
    This disallows uv from installing these dependencies itself, and  ensures it uses the exact version of these packages in the resolution.
    This is unique amongst conda based package managers, which usually just call pip from a subprocess.

    The uv resolution is included in the lock file directly.

Pixi directly supports depending on PyPI packages, the PyPA calls a distributed package a 'distribution'.
There are [Source](https://packaging.python.org/en/latest/specifications/source-distribution-format/) and [Binary](https://packaging.python.org/en/latest/specifications/binary-distribution-format/) distributions both
of which are supported by pixi.
These distributions are installed into the environment after the conda environment has been resolved and installed.
PyPI packages are not indexed on [prefix.dev](https://prefix.dev/channels) but can be viewed on [pypi.org](https://pypi.org/).

!!! warning "Important considerations"
    - **Stability**: PyPI packages might be less stable than their conda counterparts. Prefer using conda packages in the `dependencies` table where possible.
    - **Compatibility limitation**: Currently, pixi doesn't support private PyPI repositories

#### Version specification:

These dependencies don't follow the conda matchspec specification.
The `version` is a string specification of the version according to [PEP404/PyPA](https://packaging.python.org/en/latest/specifications/version-specifiers/).
Additionally, a list of extra's can be included, which are essentially optional dependencies.
Note that this `version` is distinct from the conda MatchSpec type.
See the example below to see how this is used in practice:

```toml
[dependencies]
# When using pypi-dependencies, python is needed to resolve pypi dependencies
# make sure to include this
python = ">=3.6"

[pypi-dependencies]
fastapi = "*"  # This means any version (the wildcard `*` is a pixi addition, not part of the specification)
pre-commit = "~=3.5.0" # This is a single version specifier
# Using the toml map allows the user to add `extras`
pandas = { version = ">=1.0.0", extras = ["dataframe", "sql"]}

# git dependencies
# With ssh
flask = { git = "ssh://git@github.com/pallets/flask" }
# With https and a specific revision
requests = { git = "https://github.com/psf/requests.git", rev = "0106aced5faa299e6ede89d1230bd6784f2c3660" }
# TODO: will support later -> branch = '' or tag = '' to specify a branch or tag

# You can also directly add a source dependency from a path, tip keep this relative to the root of the project.
minimal-project = { path = "./minimal-project", editable = true}

# You can also use a direct url, to either a `.tar.gz` or `.zip`, or a `.whl` file
click = { url = "https://github.com/pallets/click/releases/download/8.1.7/click-8.1.7-py3-none-any.whl" }

# You can also just the default git repo, it will checkout the default branch
pytest = { git = "https://github.com/pytest-dev/pytest.git"}
```

#### Full specification

The full specification of a PyPI dependencies that pixi supports can be split into the following fields:

##### `extras`

A list of extras to install with the package. e.g. `["dataframe", "sql"]`
The extras field works with all other version specifiers as it is an addition to the version specifier.

```toml
pandas = { version = ">=1.0.0", extras = ["dataframe", "sql"]}
pytest = { git = "URL", extras = ["dev"]}
black = { url = "URL", extras = ["cli"]}
minimal-project = { path = "./minimal-project", editable = true, extras = ["dev"]}
```

##### `version`

The version of the package to install. e.g. `">=1.0.0"` or `*` which stands for any version, this is pixi specific.
Version is our default field so using no inline table (`{}`) will default to this field.

```toml
py-rattler = "*"
ruff = "~=1.0.0"
pytest = {version = "*", extras = ["dev"]}
```

##### `git`

A git repository to install from.
This support both https:// and ssh:// urls.

Use `git` in combination with `rev` or `subdirectory`:

- `rev`: A specific revision to install. e.g. `rev = "0106aced5faa299e6ede89d1230bd6784f2c3660`
- `subdirectory`: A subdirectory to install from. `subdirectory = "src"` or `subdirectory = "src/packagex"`

```toml
# Note don't forget the `ssh://` or `https://` prefix!
pytest = { git = "https://github.com/pytest-dev/pytest.git"}
requests = { git = "https://github.com/psf/requests.git", rev = "0106aced5faa299e6ede89d1230bd6784f2c3660" }
py-rattler = { git = "ssh://git@github.com:mamba-org/rattler.git", subdirectory = "py-rattler" }
```

##### `path`

A local path to install from. e.g. `path = "./path/to/package"`
We would advise to keep your path projects in the project, and to use a relative path.

Set `editable` to `true` to install in editable mode, this is highly recommended as it is hard to reinstall if you're not using editable mode. e.g. `editable = true`

```toml
minimal-project = { path = "./minimal-project", editable = true}
```

##### `url`

A URL to install a wheel or sdist directly from an url.

```toml
pandas = {url = "https://files.pythonhosted.org/packages/3d/59/2afa81b9fb300c90531803c0fd43ff4548074fa3e8d0f747ef63b3b5e77a/pandas-2.2.1.tar.gz"}
```

??? tip "Did you know you can use: `add --pypi`?"
    Use the `--pypi` flag with the `add` command to quickly add PyPI packages from the CLI.
    E.g `pixi add --pypi flask`

    _This does not support all the features of the `pypi-dependencies` table yet._

#### Source dependencies (`sdist`)

The [Source Distribution Format](https://packaging.python.org/en/latest/specifications/source-distribution-format/) is a source based format (sdist for short), that a package can include alongside the binary wheel format.
Because these distributions need to be built, the need a python executable to do this.
This is why python needs to be present in a conda environment.
Sdists usually depend on system packages to be built, especially when compiling C/C++ based python bindings.
Think for example of Python SDL2 bindings depending on the C library: SDL2.
To help built these dependencies we activate the conda environment that includes these pypi dependencies before resolving.
This way when a source distribution depends on `gcc` for example, it's used from the conda environment instead of the system.

### `host-dependencies`

This table contains dependencies that are needed to build your project but which should not be included when your project is installed as part of another project.
In other words, these dependencies are available during the build but are no longer available when your project is installed.
Dependencies listed in this table are installed for the architecture of the target machine.

```toml
[host-dependencies]
python = "~=3.10.3"
```

Typical examples of host dependencies are:

- Base interpreters: a Python package would list `python` here and an R package would list `mro-base` or `r-base`.
- Libraries your project links against during compilation like `openssl`, `rapidjson`, or `xtensor`.

### `build-dependencies`

This table contains dependencies that are needed to build the project.
Different from `dependencies` and `host-dependencies` these packages are installed for the architecture of the _build_ machine.
This enables cross-compiling from one machine architecture to another.

```toml
[build-dependencies]
cmake = "~=3.24"
```

Typical examples of build dependencies are:

- Compilers are invoked on the build machine, but they generate code for the target machine.
  If the project is cross-compiled, the architecture of the build and target machine might differ.
- `cmake` is invoked on the build machine to generate additional code- or project-files which are then include in the compilation process.

!!! info
    The _build_ target refers to the machine that will execute the build.
    Programs and libraries installed by these dependencies will be executed on the build machine.

    For example, if you compile on a MacBook with an Apple Silicon chip but target Linux x86_64 then your *build* platform is `osx-arm64` and your *host* platform is `linux-64`.

## The `activation` table

If you want to run an activation script inside the environment when either doing a `pixi run` or `pixi shell` these can be defined here.
The scripts defined in this table will be sourced when the environment is activated using `pixi run` or `pixi shell`

!!! note
    The activation scripts are run by the system shell interpreter as they run before an environment is available.
    This means that it runs as `cmd.exe` on windows and `bash` on linux and osx (Unix).
    Only `.sh`, `.bash` and `.bat` files are supported.

    If you have scripts per platform use the [target](#the-target-table) table.

```toml
[activation]
scripts = ["env_setup.sh"]
# To support windows platforms as well add the following
[target.win-64.activation]
scripts = ["env_setup.bat"]
```

## The `target` table

The target table is a table that allows for platform specific configuration.
Allowing you to make different sets of tasks or dependencies per platform.

The target table is currently implemented for the following sub-tables:

- [`activation`](#the-activation-table)
- [`dependencies`](#dependencies)
- [`tasks`](#the-tasks-table)

The target table is defined using `[target.PLATFORM.SUB-TABLE]`.
E.g `[target.linux-64.dependencies]`

The platform can be any of:

- `win`, `osx`, `linux` or `unix` (`unix` matches `linux` and `osx`)
- or any of the (more) specific [target platforms](#platforms), e.g. `linux-64`, `osx-arm64`

The sub-table can be any of the specified above.

To make it a bit more clear, let's look at an example below.
Currently, pixi combines the top level tables like `dependencies` with the target-specific ones into a single set.
Which, in the case of dependencies, can both add or overwrite dependencies.
In the example below, we have `cmake` being used for all targets but on `osx-64` or `osx-arm64` a different version of python will be selected.

```toml
[dependencies]
cmake = "3.26.4"
python = "3.10"

[target.osx.dependencies]
python = "3.11"
```

Here are some more examples:

```toml
[target.win-64.activation]
scripts = ["setup.bat"]

[target.win-64.dependencies]
msmpi = "~=10.1.1"

[target.win-64.build-dependencies]
vs2022_win-64 = "19.36.32532"

[target.win-64.tasks]
tmp = "echo $TEMP"

[target.osx-64.dependencies]
clang = ">=16.0.6"
```

## The `feature` and `environments` tables

The `feature` table allows you to define features that can be used to create different `[environments]`.
The `[environments]` table allows you to define different environments. The design is explained in the [this design document](../features/multi_environment.md).

```toml title="Simplest example"
[feature.test.dependencies]
pytest = "*"

[environments]
test = ["test"]
```

This will create an environment called `test` that has `pytest` installed.

### The `feature` table

The `feature` table allows you to define the following fields per feature.

- `dependencies`: Same as the [dependencies](#dependencies).
- `pypi-dependencies`: Same as the [pypi-dependencies](#pypi-dependencies-beta-feature).
- `system-requirements`: Same as the [system-requirements](#the-system-requirements-table).
- `activation`: Same as the [activation](#the-activation-table).
- `platforms`: Same as the [platforms](#platforms). When adding features together the intersection of the platforms is taken. Be aware that the `default` feature is always implied thus this must contain all platforms the project can support.
- `channels`: Same as the [channels](#channels). Adding the `priority` field to the channels to allow concatenation of channels instead of overwriting.
- `target`: Same as the [target](#the-target-table).
- `tasks`: Same as the [tasks](#the-tasks-table).

These tables are all also available without the `feature` prefix.
When those are used we call them the `default` feature. This is a protected name you can not use for your own feature.

```toml title="Full feature table specification"
[feature.cuda]
activation = {scripts = ["cuda_activation.sh"]}
# Results in:  ["nvidia", "conda-forge"] when the default is `conda-forge`
channels = ["nvidia"]
dependencies = {cuda = "x.y.z", cudnn = "12.0"}
pypi-dependencies = {torch = "==1.9.0"}
platforms = ["linux-64", "osx-arm64"]
system-requirements = {cuda = "12"}
tasks = { warmup = "python warmup.py" }
target.osx-arm64 = {dependencies = {mlx = "x.y.z"}}
```

```toml title="Full feature table but written as separate tables"
[feature.cuda.activation]
scripts = ["cuda_activation.sh"]

[feature.cuda.dependencies]
cuda = "x.y.z"
cudnn = "12.0"

[feature.cuda.pypi-dependencies]
torch = "==1.9.0"

[feature.cuda.system-requirements]
cuda = "12"

[feature.cuda.tasks]
warmup = "python warmup.py"

[feature.cuda.target.osx-arm64.dependencies]
mlx = "x.y.z"

# Channels and Platforms are not available as separate tables as they are implemented as lists
[feature.cuda]
channels = ["nvidia"]
platforms = ["linux-64", "osx-arm64"]
```

### The `environments` table

The `environments` table allows you to define environments that are created using the features defined in the `feature` tables.

!!! important
    `default` is always implied when creating environments.
    If you don't want to use the `default` feature you can keep all the non feature tables empty.

The environments table is defined using the following fields:

- `features: Vec<Feature>`: The features that are included in the environment set, which is also the default field in the environments.
- `solve-group: String`: The solve group is used to group environments together at the solve stage.
  This is useful for environments that need to have the same dependencies but might extend them with additional dependencies.
  For instance when testing a production environment with additional test dependencies.
  These dependencies will then be the same version in all environments that have the same solve group.
  But the different environments contain different subsets of the solve-groups dependencies set.

```toml title="Simplest example"
[environments]
test = ["test"]
```

```toml title="Full environments table specification"
[environments]
test = {features = ["test"], solve-group = "test"}
prod = {features = ["prod"], solve-group = "test"}
lint = ["lint"]
```

## Global configuration

The global configuration options are documented in the [global configuration](../advanced/global_configuration.md) section.
