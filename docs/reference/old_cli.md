
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
