
The `pixi.toml` is the workspace manifest, also known as the Pixi workspace configuration file.

A `toml` file is structured in different tables.
This document will explain the usage of the different tables.

!!! tip
    We also support the `pyproject.toml` file. It has the same structure as the `pixi.toml` file. except that you need to prepend the tables with `tool.pixi` instead of just the table name.
    For example, the `[workspace]` table becomes `[tool.pixi.workspace]`.
    There are also some small extras that are available in the `pyproject.toml` file, checkout the [pyproject.toml](../python/pyproject_toml.md) documentation for more information.

## Manifest discovery

The manifest can be found at the following locations.


| **Priority** | **Location**                                                           | **Comments**                                                                       |
|--------------|------------------------------------------------------------------------|------------------------------------------------------------------------------------|
| 6            | `--manifest-path`                                                      | Command-line argument                                                              |
| 5            | `pixi.toml`                                                            | In your current working directory.                                                 |
| 4            | `pyproject.toml`                                                       | In your current working directory.                                                 |
| 3            | `pixi.toml` or `pyproject.toml`                                        | Iterate through all parent directories. The first discovered manifest is used.     |
| 1            | `$PIXI_PROJECT_MANIFEST`                                               | If `$PIXI_IN_SHELL` is set. This happens with `pixi shell` or `pixi run`.          |


!!! note
    If multiple locations exist, the manifest with the highest priority will be used.


## The `workspace` table

The minimally required information in the `workspace` table is:

```toml
--8<-- "docs/source_files/pixi_tomls/simple_pixi.toml:project"
```

### `channels`

This is a list that defines the channels used to fetch the packages from.
If you want to use channels hosted on `anaconda.org` you only need to use the name of the channel directly.

```toml
--8<-- "docs/source_files/pixi_tomls/lots_of_channels.toml:project_channels_long"
```

Channels situated on the file system are also supported with **absolute** file paths:

```toml
--8<-- "docs/source_files/pixi_tomls/lots_of_channels.toml:project_channels_path"
```

To access private or public channels on [prefix.dev](https://prefix.dev/channels) or [Quetz](https://github.com/mamba-org/quetz) use the url including the hostname:

```toml
--8<-- "docs/source_files/pixi_tomls/main_pixi.toml:project_channels"
```

### `platforms`

Defines the list of platforms that the workspace supports.
Pixi solves the dependencies for all these platforms and puts them in the lock file (`pixi.lock`).

```toml
--8<-- "docs/source_files/pixi_tomls/main_pixi.toml:project_platforms"
```

The available platforms are listed here: [link](https://docs.rs/rattler_conda_types/latest/rattler_conda_types/enum.Platform.html)

!!! tip "Special macOS behavior"
    macOS has two platforms: `osx-64` for Intel Macs and `osx-arm64` for Apple Silicon Macs.
    To support both, include both in your platforms list.
    Fallback: If `osx-arm64` can't resolve, use `osx-64`.
    Running `osx-64` on Apple Silicon uses [Rosetta](https://developer.apple.com/documentation/apple-silicon/about-the-rosetta-translation-environment) for Intel binaries.

### `name` (optional)

The name of the workspace.
If the name is not specified, the name of the directory that contains the workspace is used.

```toml
--8<-- "docs/source_files/pixi_tomls/main_pixi.toml:project_name"
```

### `version` (optional)

The version of the workspace.
This should be a valid version based on the conda Version Spec.
See the [version documentation](https://docs.rs/rattler_conda_types/latest/rattler_conda_types/struct.Version.html), for an explanation of what is allowed in a Version Spec.

```toml
--8<-- "docs/source_files/pixi_tomls/main_pixi.toml:project_version"
```

### `authors` (optional)

This is a list of authors of the workspace.

```toml
--8<-- "docs/source_files/pixi_tomls/main_pixi.toml:project_authors"
```

### `description` (optional)

This should contain a short description of the workspace.

```toml
--8<-- "docs/source_files/pixi_tomls/main_pixi.toml:project_description"
```

### `license` (optional)

The license as a valid [SPDX](https://spdx.org/licenses/) string (e.g. MIT AND Apache-2.0)

```toml
--8<-- "docs/source_files/pixi_tomls/main_pixi.toml:project_license"
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

URL of the workspace homepage.

```toml
--8<-- "docs/source_files/pixi_tomls/main_pixi.toml:project_homepage"
```

### `repository` (optional)

URL of the workspace source repository.

```toml
--8<-- "docs/source_files/pixi_tomls/main_pixi.toml:project_repository"
```

### `documentation` (optional)

URL of the workspace documentation.

```toml
--8<-- "docs/source_files/pixi_tomls/main_pixi.toml:project_documentation"
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

### `channel-priority` (optional)

This is the setting for the priority of the channels in the solver step.

Options:

- `strict`: **Default**, The channels are used in the order they are defined in the `channels` list.
    Only packages from the first channel that has the package are used.
    This ensures that different variants for a single package are not mixed from different channels.
    Using packages from different incompatible channels like `conda-forge` and `main` can lead to hard to debug ABI incompatibilities.

    We strongly recommend not to switch the default.

- `disabled`: There is no priority, all package variants from all channels will be set per package name and solved as one.
   Care should be taken when using this option.
   Since package variants can come from _any_ channel when you use this mode, packages might not be compatible.
   This can cause hard to debug ABI incompatibilities.

   We strongly discourage using this option.

```toml
channel-priority = "disabled"
```
!!! warning "`channel-priority = "disabled"` is a security risk"
    Disabling channel priority may lead to unpredictable dependency resolutions.
    This is a possible security risk as it may lead to packages being installed from unexpected channels.
    It's advisable to maintain the default strict setting and order channels thoughtfully.
    If necessary, specify a channel directly for a dependency.
    ```toml
    [workspace]
    # Putting conda-forge first solves most issues
    channels = ["conda-forge", "channel-name"]
    [dependencies]
    package = {version = "*", channel = "channel-name"}
    ```

### `requires-pixi` (optional)

The required version spec for `pixi` itself to resolve and build the workspace. If unset (**Default**),
any version is ok. If set, it must be a string to a valid conda version spec, and the version of
a running `pixi` must match the required spec before resolving or building the workspace, or exit with
an error when not match.

For example, with the following manifest, `pixi shell` will fail on `pixi 0.39.0`, but success after
upgrading to `pixi 0.40.0`:

```toml
[workspace]
requires-pixi = ">=0.40"
```

The upper bound can also be limit like this:

```toml
[workspace]
requires-pixi = ">=0.40,<1.0"
```

!!! note
    This option should be used to improve the reproducibility of building the workspace. A complicated
    requirement spec may be an obstacle to setup the building environment.


### `exclude-newer` (optional)

When specified this will exclude any package from consideration that is newer than the specified date.
This is useful to reproduce installations regardless of new package releases.

The date may be specified in the following formats:

* As an [RFC 3339](https://www.rfc-editor.org/rfc/rfc3339.html) timestamp (e.g. `2023-10-01T00:00:00Z`)
* As a date in the format `YYYY-MM-DD` (e.g. `2023-10-01`) in the systems time zone.

Both PyPi and conda packages are considered.

!! note Note that for Pypi package indexes the package index must support the `upload-time` field as specified in [`PEP 700`](https://peps.python.org/pep-0700/).
If the field is not present for a given distribution, the distribution will be treated as unavailable. PyPI provides `upload-time` for all packages.

### `build-variants` (optional)

!!! warning "Preview Feature"
    Build variants require the `pixi-build` preview feature to be enabled:
    ```toml
    [workspace]
    preview = ["pixi-build"]
    ```

Build variants allow you to specify different dependency versions for building packages in your workspace, creating a "build matrix" that targets multiple configurations. This is particularly useful for testing packages against different compiler versions, Python versions, or other critical dependencies.

Build variants are defined as key-value pairs where each key represents a dependency name and the value is a list of version specifications to build against.

#### Basic Usage

```toml
[workspace.build-variants]
python = ["3.11.*", "3.12.*"]
c_compiler_version = ["11.4", "14.0"]
```

#### How Build Variants Work

When build variants are specified, Pixi will:

1. **Create variant combinations**: Generate all possible combinations of the specified variants
2. **Build separate packages**: Create distinct package builds for each variant combination  
3. **Resolve dependencies**: Ensure each variant resolves with compatible dependency versions
4. **Generate unique build strings**: Each variant gets a unique build identifier in the package name

#### Platform-Specific Variants

Build variants can also be specified per-platform:

```toml
[workspace.build-variants]
python = ["3.11.*", "3.12.*"]

# Windows-specific variants
[workspace.target.win-64.build-variants]
python = ["3.11.*"]  # Only Python 3.11 on Windows
c_compiler = ["vs2019"]

# Linux-specific variants  
[workspace.target.linux-64.build-variants]
c_compiler = ["gcc"]
c_compiler_version = ["11.4", "13.0"]
```

#### Common Use Cases

- **Multi-version Python packages**: Build against Python 3.11 and 3.12
- **Compiler variants**: Test with different compiler versions for C/C++ packages
- **Dependency compatibility**: Ensure packages work with different versions of key dependencies
- **Cross-platform builds**: Different build configurations per operating system

For detailed examples and tutorials, see the [build variants documentation](../build/variants.md).

## The `tasks` table

Tasks are a way to automate certain custom commands in your workspace.
For example, a `lint` or `format` step.
Tasks in a Pixi workspace are essentially cross-platform shell commands, with a unified syntax across platforms.
For more in-depth information, check the [Advanced tasks documentation](../workspace/advanced_tasks.md).
Pixi's tasks are run in a Pixi environment using `pixi run` and are executed using the [`deno_task_shell`](../workspace/advanced_tasks.md#our-task-runner-deno_task_shell).

```toml
[tasks]
simple = "echo This is a simple task"
cmd = { cmd="echo Same as a simple task but now more verbose"}
depending = { cmd="echo run after simple", depends-on="simple"}
alias = { depends-on=["depending"]}
download = { cmd="curl -o file.txt https://example.com/file.txt" , outputs=["file.txt"]}
build = { cmd="npm build", cwd="frontend", inputs=["frontend/package.json", "frontend/*.js"]}
run = { cmd="python run.py $ARGUMENT", env={ ARGUMENT="value" }}
format = { cmd="black $INIT_CWD" } # runs black where you run pixi run format
clean-env = { cmd = "python isolated.py", clean-env = true} # Only on Unix!
```

You can modify this table using [`pixi task`](cli/pixi/task.md).
!!! note
    Specify different tasks for different platforms using the [target](#the-target-table) table

!!! info
    If you want to hide a task from showing up with `pixi task list` or `pixi info`, you can prefix the name with `_`.
    For example, if you want to hide `depending`, you can rename it to `_depending`.

## The `system-requirements` table

The system requirements are used to define minimal system specifications used during dependency resolution.

For example, we can define a unix system with a specific minimal libc version.
```toml
[system-requirements]
libc = "2.28"
```
or make the workspace depend on a specific version of `cuda`:
```toml
[system-requirements]
cuda = "12"
```

The options are:

- `linux`: The minimal version of the linux kernel.
- `libc`: The minimal version of the libc library. Also allows specifying the family of the libc library.
e.g. `libc = { family="glibc", version="2.28" }`
- `macos`: The minimal version of the macOS operating system.
- `cuda`: The minimal version of the CUDA library.

More information in the [system requirements documentation](../workspace/system_requirements.md).

## The `pypi-options` table

The `pypi-options` table is used to define options that are specific to PyPI registries.
These options can be specified either at the root level, which will add it to the default options feature, or on feature level, which will create a union of these options when the features are included in the environment.

The options that can be defined are:

- `index-url`: replaces the main index url.
- `extra-index-urls`: adds an extra index url.
- `find-links`: similar to `--find-links` option in `pip`.
- `no-build-isolation`: disables build isolation, can only be set per package.
- `no-build`: don't build source distributions.
- `no-binary`: don't use pre-build wheels.
- `index-strategy`: allows for specifying the index strategy to use.

These options are explained in the sections below. Most of these options are taken directly or with slight modifications from the [uv settings](https://docs.astral.sh/uv/reference/settings/). If any are missing that you need feel free to create an issue [requesting](https://github.com/prefix-dev/pixi/issues) them.


### Alternative registries

!!! info "Strict Index Priority"
    Unlike pip, because we make use of uv, we have a strict index priority. This means that the first index is used where a package can be found.
    The order is determined by the order in the toml file. Where the `extra-index-urls` are preferred over the `index-url`. Read more about this on the [uv docs](https://docs.astral.sh/uv/pip/compatibility/#packages-that-exist-on-multiple-indexes)

Often you might want to use an alternative or extra index for your workspace. This can be done by adding the `pypi-options` table to your `pixi.toml` file, the following options are available:

- `index-url`: replaces the main index url. If this is not set the default index used is `https://pypi.org/simple`.
   **Only one** `index-url` can be defined per environment.
- `extra-index-urls`: adds an extra index url. The urls are used in the order they are defined. And are preferred over the `index-url`. These are merged across features into an environment.
- `find-links`: which can either be a path `{path = './links'}` or a url `{url = 'https://example.com/links'}`.
   This is similar to the `--find-links` option in `pip`. These are merged across features into an environment.

An example:

```toml
[pypi-options]
index-url = "https://pypi.org/simple"
extra-index-urls = ["https://example.com/simple"]
find-links = [{path = './links'}]
```

There are some [examples](https://github.com/prefix-dev/pixi/tree/main/examples/pypi-custom-registry) in the Pixi repository, that make use of this feature.

!!! tip "Authentication Methods"
    To read about existing authentication methods for private registries, please check the [PyPI Authentication](../deployment/authentication.md#pypi-authentication) section.


### No Build Isolation
Even though build isolation is a good default.
One can choose to **not** isolate the build for a certain package name, this allows the build to access the `pixi` environment.
This is convenient if you want to use `torch` or something similar for your build-process.


```toml
[dependencies]
pytorch = "2.4.0"

[pypi-options]
no-build-isolation = ["detectron2"]

[pypi-dependencies]
detectron2 = { git = "https://github.com/facebookresearch/detectron2.git", rev = "5b72c27ae39f99db75d43f18fd1312e1ea934e60"}
```

Setting `no-build-isolation` also affects the order in which PyPI packages are installed.
Packages are installed in that order:
- conda packages in one go
- packages with build isolation in one go
- packages without build isolation installed in the order they are added to `no-build-isolation`

It is also possible to remove all packages from build isolation by setting the `no-build-isolation` to `true`.

```toml
[pypi-options]
no-build-isolation = true
```

!!! tip "Conda dependencies define the build environment"
    To use `no-build-isolation` effectively, use conda dependencies to define the build environment. These are installed before the PyPI dependencies are resolved, this way these dependencies are available during the build process. In the example above adding `torch` as a PyPI dependency would be ineffective, as it would not yet be installed during the PyPI resolution phase.

### No Build
When enabled, resolving will not run arbitrary Python code. The cached wheels of already-built source distributions will be reused, but operations that require building distributions will exit with an error.

Can be either set per package or globally.
```toml
[pypi-options]
# No sdists allowed
no-build = true # default is false
```
or:
```toml
[pypi-options]
no-build = ["package1", "package2"]
```

When features are merged, the following priority is adhered:
`no-build = true` > `no-build = ["package1", "package2"]` > `no-build = false`
So, to expand: if `no-build = true` is set for *any* feature in the environment, this will be used as the setting for the environment.


### No Binary
Don't install pre-built wheels.

The given packages will be built and installed from source. The resolver will still use pre-built wheels to extract package metadata, if available.

Can be either set per package or globally.

```toml
[pypi-options]
# Never use pre-build wheels
no-binary = true # default is false
```
or:
```toml
[pypi-options]
no-binary = ["package1", "package2"]
```

When features are merged, the following priority is adhered:
`no-binary = true` > `no-binary = ["package1", "package2"]` > `no-binary = false`
So, to expand: if `no-binary = true` is set for *any* feature in the environment, this will be used as the setting for the environment.


### Index Strategy

The strategy to use when resolving against multiple index URLs. Description modified from the [uv](https://docs.astral.sh/uv/reference/settings/#index-strategy) documentation:

By default, `uv` and thus `pixi`, will stop at the first index on which a given package is available, and limit resolutions to those present on that first index (first-match). This prevents *dependency confusion* attacks, whereby an attack can upload a malicious package under the same name to a secondary index.

!!! warning "One index strategy per environment"
    Only one `index-strategy` can be defined per environment or solve-group, otherwise, an error will be shown.

#### Possible values:

- **"first-index"**: Only use results from the first index that returns a match for a given package name
- **"unsafe-first-match"**: Search for every package name across all indexes, exhausting the versions from the first index before moving on to the next. Meaning if the package `a` is available on index `x` and `y`, it will prefer the version from `x` unless you've requested a package version that is **only** available on `y`.
- **"unsafe-best-match"**: Search for every package name across all indexes, preferring the *best* version found. If a package version is in multiple indexes, only look at the entry for the first index. So given index, `x` and `y` that both contain package `a`, it will take the *best* version from either `x` or `y`, but should **that version** be available on both indexes it will prefer `x`.

!!! info "PyPI only"
    The `index-strategy` only changes PyPI package resolution and not conda package resolution.

## The `dependencies` table(s)
??? info "Details regarding the dependencies"
    For more detail regarding the dependency types, make sure to check the [Run, Host, Build](../build/dependency_types.md) dependency documentation.

This section defines what dependencies you would like to use for your workspace.

There are multiple dependencies tables.
The default is `[dependencies]`, which are dependencies that are shared across platforms.

Dependencies are defined using a [VersionSpec](https://docs.rs/rattler_conda_types/latest/rattler_conda_types/version_spec/enum.VersionSpec.html).
A `VersionSpec` combines a [Version](https://docs.rs/rattler_conda_types/latest/rattler_conda_types/struct.Version.html) with an optional operator.


Some examples are:

```toml
# Use this exact package version
package0 = "==1.2.3"
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
Don't forget to add the channel to the `workspace` table should you use anything different than `conda-forge`.
Even if the dependency defines a channel that channel should be added to the `workspace.channels` list.

```toml
[dependencies]
python = ">3.9,<=3.11"
rust = "==1.72"
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
httpx = { git = "https://github.com/encode/httpx.git", rev = "c7c13f18a5af4c64c649881b2fe8dbd72a519c32"}

# With https and a specific branch
boltons = { git = "https://github.com/mahmoud/boltons.git", branch = "master" }

# With https and a specific tag
boltons = { git = "https://github.com/mahmoud/boltons.git", tag = "25.0.0" }

# With https, specific tag and some subdirectory
boltons = { git = "https://github.com/mahmoud/boltons.git", tag = "25.0.0", subdirectory = "some-subdir" }

# You can also directly add a source dependency from a path, tip keep this relative to the root of the workspace.
minimal-project = { path = "./minimal-project", editable = true}

# You can also use a direct url, to either a `.tar.gz` or `.zip`, or a `.whl` file
click = { url = "https://github.com/pallets/click/releases/download/8.1.7/click-8.1.7-py3-none-any.whl" }

# You can also just the default git repo, it will checkout the default branch
pytest = { git = "https://github.com/pytest-dev/pytest.git"}
```

!!! warning "Using git SSH URLs"
    When using SSH URLs in git dependencies, make sure to have your SSH key added to your SSH agent.
    You can do this by running `ssh-add` which will prompt you for your SSH key passphrase. Make sure that the `ssh-add` agent or service is running and you have a generated public/private SSH key. For more details on how to do this, check the [Github SSH documentation](https://docs.github.com/en/authentication/connecting-to-github-with-ssh/generating-a-new-ssh-key-and-adding-it-to-the-ssh-agent).

#### Full specification

The full specification of a PyPI dependencies that Pixi supports can be split into the following fields:

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

The version of the package to install. e.g. `">=1.0.0"` or `*` which stands for any version, this is Pixi specific.
Version is our default field so using no inline table (`{}`) will default to this field.

```toml
py-rattler = "*"
ruff = "~=1.0.0"
pytest = {version = "*", extras = ["dev"]}
```

##### `index`

The index parameter allows you to specify the URL of a custom package index for the installation of a specific package.
This feature is useful when you want to ensure that a package is retrieved from a particular source, rather than from the default index.

For example, to use some other than the official Python Package Index (PyPI) at https://pypi.org/simple, you can use the `index` parameter:

```toml
torch = { version = "*", index = "https://download.pytorch.org/whl/cu118" }
```

This is useful for PyTorch specifically, as the registries are pinned to different CUDA versions.
Learn more about installing PyTorch [here](../python/pytorch.md).

##### `git`

A git repository to install from.
This support both https:// and ssh:// urls.

Use `git` in combination with `rev` or `subdirectory`:

- `rev`: A specific revision to install. e.g. `rev = "0106aced5faa299e6ede89d1230bd6784f2c3660`
- `subdirectory`: A subdirectory to install from. `subdirectory = "src"` or `subdirectory = "src/packagex"`

```toml
# Note don't forget the `ssh://` or `https://` prefix!
pytest = { git = "https://github.com/pytest-dev/pytest.git"}
httpx = { git = "https://github.com/encode/httpx.git", rev = "c7c13f18a5af4c64c649881b2fe8dbd72a519c32"}
py-rattler = { git = "ssh://git@github.com/conda/rattler.git", subdirectory = "py-rattler" }
```

##### `path`

A local path to install from. e.g. `path = "./path/to/package"`
We would advise to keep your path projects in the workspace, and to use a relative path.

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
## The `activation` table

The activation table is used for specialized activation operations that need to be run when the environment is activated.

There are two types of activation operations a user can modify in the manifest:

- `scripts`: A list of scripts that are run when the environment is activated.
- `env`: A mapping of environment variables that are set when the environment is activated.

These activation operations will be run before the `pixi run` and `pixi shell` commands.

!!! note
    The script specified in the `scripts` section are not directly sourced in the `pixi shell`, but rather they are called,
    and the environment variables they set are then set in the `pixi shell`, so any defined function or other non-environment variable
    modification to the environment will be ignored.

!!! note
    The activation operations are run by the system shell interpreter as they run before an environment is available.
    This means that it runs as `cmd.exe` on windows and `bash` on linux and osx (Unix).
    Only `.sh`, `.bash` and `.bat` files are supported.

    And the environment variables are set in the shell that is running the activation script, thus take note when using e.g. `$` or `%`.

    If you have scripts or env variable per platform use the [target](#the-target-table) table.

```toml
[activation]
scripts = ["env_setup.sh"]
env = { ENV_VAR = "value" }

# To support windows platforms as well add the following
[target.win-64.activation]
scripts = ["env_setup.bat"]

[target.linux-64.activation.env]
ENV_VAR = "linux-value"

# You can also reference existing environment variables, but this has
# to be done separately for unix-like operating systems and Windows
[target.unix.activation.env]
ENV_VAR = "$OTHER_ENV_VAR/unix-value"

[target.win.activation.env]
ENV_VAR = "%OTHER_ENV_VAR%\\windows-value"
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
Currently, Pixi combines the top level tables like `dependencies` with the target-specific ones into a single set.
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
The `[environments]` table allows you to define different environments. The design is explained in the [this design document](../workspace/multi_environment.md).

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
- `pypi-dependencies`: Same as the [pypi-dependencies](#pypi-dependencies).
- `pypi-options`: Same as the [pypi-options](#the-pypi-options-table).
- `system-requirements`: Same as the [system-requirements](#the-system-requirements-table).
- `activation`: Same as the [activation](#the-activation-table).
- `platforms`: Same as the [platforms](#platforms). Unless overridden, the `platforms` of the feature will be those defined at workspace level.
- `channels`: Same as the [channels](#channels). Unless overridden, the `channels` of the feature will be those defined at workspace level.
- `channel-priority`: Same as the [channel-priority](#channel-priority-optional).
- `target`: Same as the [target](#the-target-table).
- `tasks`: Same as the [tasks](#the-tasks-table).

These tables are all also available without the `feature` prefix.
When those are used we call them the `default` feature. This is a protected name you can not use for your own feature.

```toml title="Cuda feature table example"
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

```toml title="Cuda feature table example but written as separate tables"
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

The `[environments]` table allows you to define environments that are created using the features defined in the `[feature]` tables.

The environments table is defined using the following fields:

- `features`: The features that are included in the environment. Unless `no-default-feature` is set to `true`, the default feature is implicitly included in the environment.
- `solve-group`: The solve group is used to group environments together at the solve stage.
  This is useful for environments that need to have the same dependencies but might extend them with additional dependencies.
  For instance when testing a production environment with additional test dependencies.
  These dependencies will then be the same version in all environments that have the same solve group.
  But the different environments contain different subsets of the solve-groups dependencies set.
- `no-default-feature`: Whether to include the default feature in that environment. The default is `false`, to include the default feature.

```toml title="Full environments table specification"
[environments]
test = {features = ["test"], solve-group = "test"}
prod = {features = ["prod"], solve-group = "test"}
lint = {features = ["lint"], no-default-feature = true}
```
As shown in the example above, in the simplest of cases, it is possible to define an environment only by listing its features:

```toml title="Simplest example"
[environments]
test = ["test"]
```

is equivalent to

```toml title="Simplest example expanded"
[environments]
test = {features = ["test"]}
```

When an environment comprises several features (including the default feature):

- The `activation` and `tasks` of the environment are the union of the `activation` and `tasks` of all its features.
- The `dependencies` and `pypi-dependencies` of the environment are the union of the `dependencies` and `pypi-dependencies` of all its features. This means that if several features define a requirement for the same package, both requirements will be combined. Beware of conflicting requirements across features added to the same environment.
- The `system-requirements` of the environment is the union of the `system-requirements` of all its features. If multiple features specify a requirement for the same system package, the highest version is chosen.
- The `channels` of the environment is the union of the `channels` of all its features. Channel priorities can be specified in each feature, to ensure channels are considered in the right order in the environment.
- The `platforms` of the environment is the intersection of the `platforms` of all its features. Be aware that the platforms supported by a feature (including the default feature) will be considered as the `platforms` defined at workspace level (unless overridden in the feature). This means that it is usually a good idea to set the workspace `platforms` to all platforms it can support across its environments.

## Global configuration

The global configuration options are documented in the [global configuration](../reference/pixi_configuration.md) section.


## Preview features
Pixi sometimes introduces new features that are not yet stable, but that we would like for users to test out. These features are called preview features. Preview features are disabled by default and can be enabled by setting the `preview` field in the workspace manifest. The preview field is an array of strings that specify the preview features to enable, or the boolean value `true` to enable all preview features.

An example of a preview feature in the manifest:

```toml
--8<-- "docs/source_files/pixi_tomls/simple_pixi_build.toml:preview"
```

Preview features in the documentation will be marked as such on the relevant pages.


## The `package` section

!!! warning "Important note"
    `pixi-build` is a [preview feature](#preview-features), and will change until it is stabilized.
    Please keep that in mind when you use it for your workspaces.
    ```toml
    --8<-- "docs/source_files/pixi_tomls/simple_pixi_build.toml:preview"
    ```

The package section can be added
to a workspace manifest to define the package that is built by Pixi.

A package section needs to be inside a `workspace`, 
either in the same manifest file as the `[workspace]` table or in a sub folder `pixi.toml`/`pyproject.toml` file.

These packages will be built into a conda package that can be installed into a conda environment.
The package section is defined using the following fields:

- `name`: The name of the package.
- `version`: The version of the package.
- `build`: The build system used to build the package.
- `build-dependencies`: The build dependencies of the package.
- `host-dependencies`: The host dependencies of the package.
- `run-dependencies`: The run dependencies of the package.
- `target`: The target table to configure target specific dependencies. (Similar to the [target](#the-target-table) table)

And to extend the basics, it can also contain the following fields:

- `description`: A short description of the package.
- `authors`: A list of authors of the package.
- `license`: The license of the package.
- `license-file`: The license file of the package.
- `readme`: The README file of the package.
- `homepage`: The homepage link of the package.
- `repository`: The repository link of the package.
- `documentation`: The documentation link of the package.

```toml
--8<-- "docs/source_files/pixi_tomls/pixi-package-manifest.toml:package"
--8<-- "docs/source_files/pixi_tomls/pixi-package-manifest.toml:extra-fields"
```

!!! note "Workspace inheritance"
    Most extra fields can be inherited from the workspace manifest.
    This means that you can define the `description`, `authors`, `license` in the workspace manifest, and they will be inherited by the package manifest.
    ```toml
    [workspace]    
    name = "my-workspace"
    
    [package]
    name = { workspace = true } # Inherit the name from the workspace
    ```

### `build` table

The build system specifies how the package can be built.
The build system is a table that can contain the following fields:

- `source`: specifies the location of the source code for the package. Default: manifest directory. Currently supported options:
  - `path`: a string representing a relative or absolute path to the source code.
- `channels`: specifies the channels to get the build backend from.
- `backend`: specifies the build backend to use. This is a table that can contain the following fields:
  - `name`: the name of the build backend to use. This will also be the executable name.
  - `version`: the version of the build backend to use.
- `configuration`: a table that contains the configuration options for the build backend.
- `target`: a table that can contain target specific build configuration.

More documentation on the backends can be found in the [build backend documentation](../build/backends.md).

```toml
--8<-- "docs/source_files/pixi_tomls/pixi-package-manifest.toml:build-system"
```


### The `build` `host` and `run` dependencies tables
The dependencies of a package are split into three tables.
Each of these tables has a different purpose and is used to define the dependencies of the package.

- [`build-dependencies`](#build-dependencies): Dependencies that are required to build the package on the build platform.
- [`host-dependencies`](#host-dependencies): Dependencies that are required during the build process, to link against the package on the target platform.
- [`run-dependencies`](#run-dependencies): Dependencies that are required to run the package on the target platform.


### `build-dependencies`
Build dependencies are required in the build environment and contain all tools that are not needed on the host of the package.

Following packages are examples of typical build dependencies:

- compilers (`gcc`, `clang`, `gfortran`)
- build tools (`cmake`, `ninja`, `meson`)
- `make`
- `pkg-config`
- VSC packages (`git`, `svn`)

??? warning "Using git SSH URLs"
    When using SSH URLs in git dependencies, make sure to have your SSH key added to your SSH agent.
    You can do this by running `ssh-add` which will prompt you for your SSH key passphrase.
    Make sure that the `ssh-add` agent or service is running and you have a generated public/private SSH key.
    For more details on how to do this, check the [Github SSH documentation](https://docs.github.com/en/authentication/connecting-to-github-with-ssh/generating-a-new-ssh-key-and-adding-it-to-the-ssh-agent).


```toml
--8<-- "docs/source_files/pixi_tomls/pixi-package-manifest.toml:build-dependencies"
```

### `host-dependencies`

Host dependencies are required during build phase, but in contrast to build packages they have to be present on the host.

Following packages are typical examples for host dependencies:

- shared libraries (c/c++)
- python/r libraries that link against c libraries
- `python`, `r-base`
- `setuptools`, `pip`

```toml
--8<-- "docs/source_files/pixi_tomls/pixi-package-manifest.toml:host-dependencies"
```

### `run-dependencies`
The `run-dependencies` are the packages that will be installed in the environment when the package is run.

- Libraries
- Extra data file packages
- Python/R packages that are not needed during build time

```toml
--8<-- "docs/source_files/pixi_tomls/pixi-package-manifest.toml:run-dependencies"
```
