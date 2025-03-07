
## `global`

Global is the main entry point for the part of pixi that executes on the global(system) level.
All commands in this section are used to manage global installations of packages and environments through the global manifest.
More info on the global manifest can be found [here](../global_tools/introduction.md).

!!! tip
    Binaries and environments installed globally are stored in `~/.pixi`
    by default, this can be changed by setting the `PIXI_HOME` environment
    variable.
### `global add`

Adds dependencies to a global environment.
Without exposing the binaries of that package to the system by default.

##### Arguments
1. `[PACKAGE]`: The packages to add, this excepts the matchspec format. (e.g. `python=3.9.*`, `python [version='3.11.0', build_number=1]`)

##### Options
- `--environment <ENVIRONMENT> (-e)`: The environment to install the package into.
- `--expose <EXPOSE>`: A mapping from name to the binary to expose to the system.

```shell
pixi global add python=3.9.* --environment my-env
pixi global add python=3.9.* --expose py39=python3.9 --environment my-env
pixi global add numpy matplotlib --environment my-env
pixi global add numpy matplotlib --expose np=python3.9 --environment my-env
```

### `global edit`
Edit the global manifest file in the default editor.

Will try to use the `EDITOR` environment variable, if not set it will use `nano` on Unix systems and `notepad` on Windows.

##### Arguments
1. `<EDITOR>`: The editor to use. (optional)
```shell
pixi global edit
pixi global edit code
pixi global edit vim
```

### `global install`

This command installs package(s) into its own environment and adds the binary to `PATH`.
Allowing you to access it anywhere on your system without activating the environment.

##### Arguments

1.`[PACKAGE]`: The package(s) to install, this can also be a version constraint.

##### Options

- `--channel <CHANNEL> (-c)`: specify a channel that the project uses. Defaults to `conda-forge`. (Allowed to be used more than once)
- `--platform <PLATFORM> (-p)`: specify a platform that you want to install the package for. (default: current platform)
- `--environment <ENVIRONMENT> (-e)`: The environment to install the package into. (default: name of the tool)
- `--expose <EXPOSE>`: A mapping from name to the binary to expose to the system. (default: name of the tool)
- `--with <WITH>`: Add additional dependencies to the environment. Their executables will not be exposed.
- `--force-reinstall`: Specifies that the packages should be reinstalled even if they are already installed
- `--no-shortcut`: Specifies that no shortcuts should be created for the installed packages

```shell
pixi global install ruff
# Multiple packages can be installed at once
pixi global install starship rattler-build
# Specify the channel(s)
pixi global install --channel conda-forge --channel bioconda trackplot
# Or in a more concise form
pixi global install -c conda-forge -c bioconda trackplot

# Support full conda matchspec
pixi global install python=3.9.*
pixi global install "python [version='3.11.0', build_number=1]"
pixi global install "python [version='3.11.0', build=he550d4f_1_cpython]"
pixi global install python=3.11.0=h10a6764_1_cpython

# Install for a specific platform, only useful on osx-arm64
pixi global install --platform osx-64 ruff

# Install a package with all its executables exposed, together with additional packages that don't expose anything
pixi global install ipython --with numpy --with scipy

# Install into a specific environment name and expose all executables
pixi global install --environment data-science ipython jupyterlab numpy matplotlib

# Expose the binary under a different name
pixi global install --expose "py39=python3.9" "python=3.9.*"
```

!!! tip
    Running `osx-64` on Apple Silicon will install the Intel binary but run it using [Rosetta](https://developer.apple.com/documentation/apple-silicon/about-the-rosetta-translation-environment)
    ```
    pixi global install --platform osx-64 ruff
    ```

After using global install, you can use the package you installed anywhere on your system.

### `global uninstall`
Uninstalls environments from the global environment.
This will remove the environment and all its dependencies from the global environment.
It will also remove the related binaries from the system.

##### Arguments
1. `[ENVIRONMENT]`: The environments to uninstall.

```shell
pixi global uninstall my-env
pixi global uninstall pixi-pack rattler-build
```

### `global remove`

Removes a package from a global environment.

##### Arguments

1. `[PACKAGE]`: The packages to remove.

##### Options

- `--environment <ENVIRONMENT> (-e)`: The environment to remove the package from.

```shell
pixi global remove -e my-env package1 package2
```


### `global list`

This command shows the current installed global environments including what binaries come with it.
A global installed package/environment can possibly contain multiple exposed binaries and they will be listed out in the command output.

##### Options
- `--environment <ENVIRONMENT> (-e)`: The environment to install the package into. (default: name of the tool)

We'll only show the dependencies and exposed binaries of the environment if they differ from the environment name.
Here is an example of a few installed packages:

```
pixi global list
```
Results in:
```
Global environments at /home/user/.pixi:
├── gh: 2.57.0
├── pixi-pack: 0.1.8
├── python: 3.11.0
│   └─ exposes: 2to3, 2to3-3.11, idle3, idle3.11, pydoc, pydoc3, pydoc3.11, python, python3, python3-config, python3.1, python3.11, python3.11-config
├── rattler-build: 0.22.0
├── ripgrep: 14.1.0
│   └─ exposes: rg
├── vim: 9.1.0611
│   └─ exposes: ex, rview, rvim, view, vim, vimdiff, vimtutor, xxd
└── zoxide: 0.9.6
```

Here is an example of list of a single environment:
```
pixi g list -e pixi-pack
```
Results in:
```
The 'pixi-pack' environment has 8 packages:
Package          Version    Build        Size
_libgcc_mutex    0.1        conda_forge  2.5 KiB
_openmp_mutex    4.5        2_gnu        23.1 KiB
ca-certificates  2024.8.30  hbcca054_0   155.3 KiB
libgcc           14.1.0     h77fa898_1   826.5 KiB
libgcc-ng        14.1.0     h69a702a_1   50.9 KiB
libgomp          14.1.0     h77fa898_1   449.4 KiB
openssl          3.3.2      hb9d3cd8_0   2.8 MiB
pixi-pack        0.1.8      hc762bcd_0   4.3 MiB
Package          Version    Build        Size

Exposes:
pixi-pack
Channels:
conda-forge
Platform: linux-64
```


### `global sync`
As the global manifest can be manually edited, this command will sync the global manifest with the current state of the global environment.
You can modify the manifest in `$HOME/manifests/pixi_global.toml`.

```shell
pixi global sync
```

### `global expose`
Modify the exposed binaries of a global environment.

#### `global expose add`
Add exposed binaries from an environment to your global environment.

##### Arguments
1. `[MAPPING]`: The binaries to expose (`python`), or give a map to expose a binary under a different name. (e.g. `py310=python3.10`)
The mapping is mapped as `exposed_name=binary_name`.
Where the exposed name is the one you will be able to use in the terminal, and the binary name is the name of the binary in the environment.

##### Options
- `--environment <ENVIRONMENT> (-e)`: The environment to expose the binaries from.

```shell
pixi global expose add python --environment my-env
pixi global expose add py310=python3.10 --environment python
```

#### `global expose remove`
Remove exposed binaries from the global environment.

##### Arguments
1. `[EXPOSED_NAME]`: The binaries to remove from the main global environment.

```shell
pixi global expose remove python
pixi global expose remove py310 python3
```

### `global update`

Update all environments or specify an environment to update to the version.

##### Arguments

1. `[ENVIRONMENT]`: The environment(s) to update.

```shell
pixi global update
pixi global update pixi-pack
pixi global update bat rattler-build
```

## `project`

This subcommand allows you to modify the project configuration through the command line interface.

##### Options

- `--manifest-path <MANIFEST_PATH>`: the path to [manifest file](pixi_manifest.md), by default it searches for one in the parent directories.

### `project channel add`

Add channels to the channel list in the project configuration.
When you add channels, the channels are tested for existence, added to the lock file and the environment is reinstalled.

##### Arguments

1. `<CHANNEL>`: The channels to add, name or URL.

##### Options

- `--no-install`: do not update the environment, only add changed packages to the lock-file.
- `--feature <FEATURE> (-f)`: The feature for which the channel is added.
- `--prepend`: Prepend the channel to the list of channels.

```
pixi project channel add robostack
pixi project channel add bioconda conda-forge robostack
pixi project channel add file:///home/user/local_channel
pixi project channel add https://repo.prefix.dev/conda-forge
pixi project channel add --no-install robostack
pixi project channel add --feature cuda nvidia
pixi project channel add --prepend pytorch
```

### `project channel list`

List the channels in the manifest file

##### Options

- `urls`: show the urls of the channels instead of the names.

```sh
$ pixi project channel list
Environment: default
- conda-forge

$ pixi project channel list --urls
Environment: default
- https://conda.anaconda.org/conda-forge/

```

### `project channel remove`

List the channels in the manifest file

##### Arguments

1. `<CHANNEL>...`: The channels to remove, name(s) or URL(s).

##### Options

- `--no-install`: do not update the environment, only add changed packages to the lock-file.
- `--feature <FEATURE> (-f)`: The feature for which the channel is removed.

```sh
pixi project channel remove conda-forge
pixi project channel remove https://conda.anaconda.org/conda-forge/
pixi project channel remove --no-install conda-forge
pixi project channel remove --feature cuda nvidia
```

### `project description get`

Get the project description.

```sh
$ pixi project description get
Package management made easy!
```

### `project description set`

Set the project description.

##### Arguments

1. `<DESCRIPTION>`: The description to set.

```sh
pixi project description set "my new description"
```

### `project environment add`

Add an environment to the manifest file.

##### Arguments

1. `<NAME>`: The name of the environment to add.

##### Options

- `-f, --feature <FEATURES>`: Features to add to the environment.
- `--solve-group <SOLVE_GROUP>`: The solve-group to add the environment to.
- `--no-default-feature`: Don't include the default feature in the environment.
- `--force`:  Update the manifest even if the environment already exists.

```sh
pixi project environment add env1 --feature feature1 --feature feature2
pixi project environment add env2 -f feature1 --solve-group test
pixi project environment add env3 -f feature1 --no-default-feature
pixi project environment add env3 -f feature1 --force
```

### `project environment remove`

Remove an environment from the manifest file.

##### Arguments

1. `<NAME>`: The name of the environment to remove.

```shell
pixi project environment remove env1
```

### `project environment list`

List the environments in the manifest file.

```shell
pixi project environment list
```

### `project export conda-environment`

Exports a conda [`environment.yml` file](https://conda.io/projects/conda/en/latest/user-guide/tasks/manage-environments.html#creating-an-environment-from-an-environment-yml-file). The file can be used to create a conda environment using conda/mamba:

```shell
pixi project export conda-environment environment.yml
mamba create --name <env> --file environment.yml
```

##### Arguments

1. `<OUTPUT_PATH>`: Optional path to render environment.yml to. Otherwise it will be printed to standard out.

##### Options

- `--environment <ENVIRONMENT> (-e)`: Environment to render.
- `--platform <PLATFORM> (-p)`: The platform to render.

```sh
pixi project export conda-environment --environment lint
pixi project export conda-environment --platform linux-64 environment.linux-64.yml
```

### `project export conda-explicit-spec`

Render a platform-specific conda [explicit specification file](https://conda.io/projects/conda/en/latest/user-guide/tasks/manage-environments.html#building-identical-conda-environments)
for an environment. The file can be then used to create a conda environment using conda/mamba:

```shell
mamba create --name <env> --file <explicit spec file>
```

As the explicit specification file format does not support pypi-dependencies, use the `--ignore-pypi-errors` option to ignore those dependencies.

##### Arguments

1. `<OUTPUT_DIR>`:  Output directory for rendered explicit environment spec files.

##### Options

- `--environment <ENVIRONMENT> (-e)`: Environment to render. Can be repeated for multiple envs. Defaults to all environments.
- `--platform <PLATFORM> (-p)`: The platform to render. Can be repeated for multiple platforms. Defaults to all platforms available for selected environments.
- `--ignore-pypi-errors`: PyPI dependencies are not supported in the conda explicit spec file. This flag allows creating the spec file even if PyPI dependencies are present.

```sh
pixi project export conda-explicit-spec output
pixi project export conda-explicit-spec -e default -e test -p linux-64 output
```

### `project name get`

Get the project name.

```sh
$ pixi project name get
my project name
```

### `project name set`

Set the project name.

##### Arguments

1. `<NAME>`: The name to set.

```sh
pixi project name set "my new project name"
```

### `project platform add`

Adds a platform(s) to the manifest file and updates the lock file.

##### Arguments

1. `<PLATFORM>...`: The platforms to add.

##### Options

- `--no-install`: do not update the environment, only add changed packages to the lock-file.
- `--feature <FEATURE> (-f)`: The feature for which the platform will be added.

```sh
pixi project platform add win-64
pixi project platform add --feature test win-64
```

### `project platform list`

List the platforms in the manifest file.

```sh
$ pixi project platform list
osx-64
linux-64
win-64
osx-arm64
```

### `project platform remove`

Remove platform(s) from the manifest file and updates the lock file.

##### Arguments

1. `<PLATFORM>...`: The platforms to remove.

##### Options

- `--no-install`: do not update the environment, only add changed packages to the lock-file.
- `--feature <FEATURE> (-f)`: The feature for which the platform will be removed.

```sh
pixi project platform remove win-64
pixi project platform remove --feature test win-64
```

### `project version get`

Get the project version.

```sh
$ pixi project version get
0.11.0
```

### `project version set`

Set the project version.

##### Arguments

1. `<VERSION>`: The version to set.

```sh
pixi project version set "0.13.0"
```

### `project version {major|minor|patch}`

Bump the project version to {MAJOR|MINOR|PATCH}.

```sh
pixi project version major
pixi project version minor
pixi project version patch
```

### `project system-requirement add`

Add a system requirement to the project configuration.

##### Arguments
1. `<REQUIREMENT>`: The name of the system requirement.
2. `<VERSION>`: The version of the system requirement.

##### Options
- `--family <FAMILY>`: The family of the system requirement. Only used for `other-libc`.
- `--feature <FEATURE> (-f)`: The feature for which the system requirement is added.

```shell
pixi project system-requirements add cuda 12.6
pixi project system-requirements add linux 5.15.2
pixi project system-requirements add macos 15.2
pixi project system-requirements add glibc 2.34
pixi project system-requirements add other-libc 1.2.3 --family musl
pixi project system-requirements add --feature cuda cuda 12.0
```

### `project system-requirement list`

List the system requirements in the project configuration.

##### Options
- `--environment <ENVIRONMENT> (-e)`: The environment to list the system requirements for.

```shell
pixi project system-requirements list
pixi project system-requirements list --environment test
```

[^1]:
    An **up-to-date** lock file means that the dependencies in the lock file are allowed by the dependencies in the manifest file.
    For example

    - a manifest with `python = ">= 3.11"` is up-to-date with a `name: python, version: 3.11.0` in the `pixi.lock`.
    - a manifest with `python = ">= 3.12"` is **not** up-to-date with a `name: python, version: 3.11.0` in the `pixi.lock`.

    Being up-to-date does **not** mean that the lock file holds the latest version available on the channel for the given dependency.
