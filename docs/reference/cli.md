---
part: pixi
title: Commands
description: All pixi cli subcommands
---

## Global options

- `--verbose (-v|vv|vvv)` Increase the verbosity of the output messages, the -v|vv|vvv increases the level of verbosity respectively.
- `--help (-h)` Shows help information, use `-h` to get the short version of the help.
- `--version (-V)`: shows the version of pixi that is used.
- `--quiet (-q)`: Decreases the amount of output.
- `--color <COLOR>`: Whether the log needs to be colored [env: `PIXI_COLOR=`] [default: `auto`] [possible values: `always`, `never`, `auto`].
  Pixi also honors the `FORCE_COLOR` and `NO_COLOR` environment variables.
  They both take precedence over `--color` and `PIXI_COLOR`.
- `--no-progress`: Disables the progress bar.[env: `PIXI_NO_PROGRESS`] [default: `false`]

## `init`

This command is used to create a new project.
It initializes a `pixi.toml` file and also prepares a `.gitignore` to prevent the environment from being added to `git`.

It also supports the [`pyproject.toml`](../advanced/pyproject_toml.md) file, if you have a `pyproject.toml` file in the directory where you run `pixi init`, it appends the pixi data to the `pyproject.toml` instead of a new `pixi.toml` file.

##### Arguments

1. `[PATH]`: Where to place the project (defaults to current path) [default: `.`]

##### Options

- `--channel <CHANNEL> (-c)`: specify a channel that the project uses. Defaults to `conda-forge`. (Allowed to be used more than once)
- `--platform <PLATFORM> (-p)`: specify a platform that the project supports. (Allowed to be used more than once)
- `--import <ENV_FILE> (-i)`: Import an existing conda environment file, e.g. `environment.yml`.
- `--format <FORMAT>`: Specify the format of the project file, either `pyproject` or `pixi`. [default: `pixi`]

!!! info "Importing an environment.yml"
  When importing an environment, the `pixi.toml` will be created with the dependencies from the environment file.
  The `pixi.lock` will be created when you install the environment.
  We don't support `git+` urls as dependencies for pip packages and for the `defaults` channel we use `main`, `r` and `msys2` as the default channels.

```shell
pixi init myproject
pixi init ~/myproject
pixi init  # Initializes directly in the current directory.
pixi init --channel conda-forge --channel bioconda myproject
pixi init --platform osx-64 --platform linux-64 myproject
pixi init --import environment.yml
pixi init --format pyproject
pixi init --format pixi
```

## `add`

Adds dependencies to the [manifest file](project_configuration.md).
It will only add if the package with its version constraint is able to work with rest of the dependencies in the project.
[More info](../features/multi_platform_configuration.md) on multi-platform configuration.

If the project manifest is a `pyproject.toml`, adding a pypi dependency will add it to the native pyproject `project.dependencies` array, or to the native `project.optional-dependencies` table if a feature is specified:

- `pixi add --pypi boto3` would add `boto3` to the `project.dependencies` array
- `pixi add --pypi boto3 --feature aws` would add `boto3` to the `project.dependencies.aws` array

These dependencies will be read by pixi as if they had been added to the pixi `pypi-dependencies` tables of the default or a named feature.

##### Arguments

1. `[SPECS]`: The package(s) to add, space separated. The version constraint is optional.

##### Options

- `--manifest-path <MANIFEST_PATH>`: the path to [manifest file](project_configuration.md), by default it searches for one in the parent directories.
- `--host`: Specifies a host dependency, important for building a package.
- `--build`: Specifies a build dependency, important for building a package.
- `--pypi`: Specifies a PyPI dependency, not a conda package.
  Parses dependencies as [PEP508](https://peps.python.org/pep-0508/) requirements, supporting extras and versions.
  See [configuration](project_configuration.md) for details.
- `--no-install`: Don't install the package to the environment, only add the package to the lock-file.
- `--no-lockfile-update`: Don't update the lock-file, implies the `--no-install` flag.
- `--platform <PLATFORM> (-p)`: The platform for which the dependency should be added. (Allowed to be used more than once)
- `--feature <FEATURE> (-f)`: The feature for which the dependency should be added.
- `--editable`: Specifies an editable dependency, only use in combination with `--pypi`.

```shell
pixi add numpy # (1)!
pixi add numpy pandas "pytorch>=1.8" # (2)!
pixi add "numpy>=1.22,<1.24" # (3)!
pixi add --manifest-path ~/myproject/pixi.toml numpy # (4)!
pixi add --host "python>=3.9.0" # (5)!
pixi add --build cmake # (6)!
pixi add --platform osx-64 clang # (7)!
pixi add --no-install numpy # (8)!
pixi add --no-lockfile-update numpy # (9)!
pixi add --feature featurex numpy # (10)!

# Add a pypi dependency
pixi add --pypi requests[security] # (11)!
pixi add --pypi Django==5.1rc1 # (12)!
pixi add --pypi "boltons>=24.0.0" --feature lint # (13)!
pixi add --pypi "boltons @ https://files.pythonhosted.org/packages/46/35/e50d4a115f93e2a3fbf52438435bb2efcf14c11d4fcd6bdcd77a6fc399c9/boltons-24.0.0-py3-none-any.whl" # (14)!
pixi add --pypi "exchangelib @ git+https://github.com/ecederstrand/exchangelib" # (15)!
pixi add --pypi "project @ file:///absolute/path/to/project" # (16)!
pixi add --pypi "project@file:///absolute/path/to/project" --editable # (17)!
```

1. This will add the `numpy` package to the project with the latest available for the solved environment.
2. This will add multiple packages to the project solving them all together.
3. This will add the `numpy` package with the version constraint.
4. This will add the `numpy` package to the project of the manifest file at the given path.
5. This will add the `python` package as a host dependency. There is currently no different behavior for host dependencies.
6. This will add the `cmake` package as a build dependency. There is currently no different behavior for build dependencies.
7. This will add the `clang` package only for the `osx-64` platform.
8. This will add the `numpy` package to the manifest and lockfile, without installing it in an environment.
9. This will add the `numpy` package to the manifest without updating the lockfile or installing it in the environment.
10. This will add the `numpy` package in the feature `featurex`.
11. This will add the `requests` package as `pypi` dependency with the `security` extra.
12. This will add the `pre-release` version of `Django` to the project as a `pypi` dependency.
13. This will add the `boltons` package in the feature `lint` as `pypi` dependency.
14. This will add the `boltons` package with the given `url` as `pypi` dependency.
15. This will add the `exchangelib` package with the given `git` url as `pypi` dependency.
16. This will add the `project` package with the given `file` url as `pypi` dependency.
17. This will add the `project` package with the given `file` url as an `editable` package as `pypi` dependency.

!!! tip
    If you want to use a non default pinning strategy, you can set it using [pixi's configuration](./pixi_configuration.md#pinning-strategy).
    ```
    pixi config set pinning-strategy no-pin --global
    ```
    The default is `semver` which will pin the dependencies to the latest major version or minor for `v0` versions.


## `install`

Installs an environment based on the [manifest file](project_configuration.md).
If there is no `pixi.lock` file or it is not up-to-date with the [manifest file](project_configuration.md), it will (re-)generate the lock file.

`pixi install` only installs one environment at a time, if you have multiple environments you can select the right one with the `--environment` flag.
If you don't provide an environment, the `default` environment will be installed.

Running `pixi install` is not required before running other commands.
As all commands interacting with the environment will first run the `install` command if the environment is not ready, to make sure you always run in a correct state.
E.g. `pixi run`, `pixi shell`, `pixi shell-hook`, `pixi add`, `pixi remove` to name a few.

##### Options
- `--manifest-path <MANIFEST_PATH>`: the path to [manifest file](project_configuration.md), by default it searches for one in the parent directories.
- `--frozen`: install the environment as defined in the lock file, doesn't update `pixi.lock` if it isn't up-to-date with [manifest file](project_configuration.md). It can also be controlled by the `PIXI_FROZEN` environment variable (example: `PIXI_FROZEN=true`).
- `--locked`: only install if the `pixi.lock` is up-to-date with the [manifest file](project_configuration.md)[^1]. It can also be controlled by the `PIXI_LOCKED` environment variable (example: `PIXI_LOCKED=true`). Conflicts with `--frozen`.
- `--environment <ENVIRONMENT> (-e)`: The environment to install, if none are provided the default environment will be used.

```shell
pixi install
pixi install --manifest-path ~/myproject/pixi.toml
pixi install --frozen
pixi install --locked
pixi install --environment lint
pixi install -e lint
```

## `update`

The `update` command checks if there are newer versions of the dependencies and updates the `pixi.lock` file and environments accordingly.
It will only update the lock file if the dependencies in the [manifest file](project_configuration.md) are still compatible with the new versions.

##### Arguments

1. `[PACKAGES]...` The packages to update, space separated. If no packages are provided, all packages will be updated.

##### Options
- `--manifest-path <MANIFEST_PATH>`: the path to [manifest file](project_configuration.md), by default it searches for one in the parent directories.
- `--environment <ENVIRONMENT> (-e)`: The environment to install, if none are provided all the environments are updated.
- `--platform <PLATFORM> (-p)`: The platform for which the dependencies should be updated.
- `--dry-run (-n)`: Only show the changes that would be made, without actually updating the lock file or environment.
- `--no-install`: Don't install the (solve) environment needed for solving pypi-dependencies.
- `--json`: Output the changes in json format.

```shell
pixi update numpy
pixi update numpy pandas
pixi update --manifest-path ~/myproject/pixi.toml numpy
pixi update --environment lint python
pixi update -e lint -e schema -e docs pre-commit
pixi update --platform osx-arm64 mlx
pixi update -p linux-64 -p osx-64 numpy
pixi update --dry-run
pixi update --no-install boto3
```

## `run`

The `run` commands first checks if the environment is ready to use.
When you didn't run `pixi install` the run command will do that for you.
The custom tasks defined in the [manifest file](project_configuration.md) are also available through the run command.

You cannot run `pixi run source setup.bash` as `source` is not available in the `deno_task_shell` commandos and not an executable.

##### Arguments

1. `[TASK]...` The task you want to run in the projects environment, this can also be a normal command. And all arguments after the task will be passed to the task.

##### Options

- `--manifest-path <MANIFEST_PATH>`: the path to [manifest file](project_configuration.md), by default it searches for one in the parent directories.
- `--frozen`: install the environment as defined in the lock file, doesn't update `pixi.lock` if it isn't up-to-date with [manifest file](project_configuration.md). It can also be controlled by the `PIXI_FROZEN` environment variable (example: `PIXI_FROZEN=true`).
- `--locked`: only install if the `pixi.lock` is up-to-date with the [manifest file](project_configuration.md)[^1]. It can also be controlled by the `PIXI_LOCKED` environment variable (example: `PIXI_LOCKED=true`). Conflicts with `--frozen`.
- `--environment <ENVIRONMENT> (-e)`: The environment to run the task in, if none are provided the default environment will be used or a selector will be given to select the right environment.
- `--clean-env`: Run the task in a clean environment, this will remove all environment variables of the shell environment except for the ones pixi sets. THIS DOESN't WORK ON `Windows`.
```shell
pixi run python
pixi run cowpy "Hey pixi user"
pixi run --manifest-path ~/myproject/pixi.toml python
pixi run --frozen python
pixi run --locked python
# If you have specified a custom task in the pixi.toml you can run it with run as well
pixi run build
# Extra arguments will be passed to the tasks command.
pixi run task argument1 argument2

# If you have multiple environments you can select the right one with the --environment flag.
pixi run --environment cuda python

# THIS DOESN'T WORK ON WINDOWS
# If you want to run a command in a clean environment you can use the --clean-env flag.
# The PATH should only contain the pixi environment here.
pixi run --clean-env "echo \$PATH"
```

!!! info
    In `pixi` the [`deno_task_shell`](https://deno.land/manual@v1.35.0/tools/task_runner#task-runner) is the underlying runner of the run command.
    Checkout their [documentation](https://deno.land/manual@v1.35.0/tools/task_runner#task-runner) for the syntax and available commands.
    This is done so that the run commands can be run across all platforms.

!!! tip "Cross environment tasks"
    If you're using the `depends-on` feature of the `tasks`, the tasks will be run in the order you specified them.
    The `depends-on` can be used cross environment, e.g. you have this `pixi.toml`:
    ??? "pixi.toml"
        ```toml
        [tasks]
        start = { cmd = "python start.py", depends-on = ["build"] }

        [feature.build.tasks]
        build = "cargo build"
        [feature.build.dependencies]
        rust = ">=1.74"

        [environments]
        build = ["build"]
        ```

        Then you're able to run the `build` from the `build` environment and `start` from the default environment.
        By only calling:
        ```shell
        pixi run start
        ```

## `exec`

Runs a command in a temporary environment disconnected from any project.
This can be useful to quickly test out a certain package or version.

Temporary environments are cached. If the same command is run again, the same environment will be reused.

??? note "Cleaning temporary environments"
    Currently, temporary environments can only be cleaned up manually.
    Environments for `pixi exec` are stored under `cached-envs-v0/` in the cache directory.
    Run `pixi info` to find the cache directory.

##### Arguments

1. `<COMMAND>`: The command to run.

#### Options:
* `--spec <SPECS> (-s)`: Matchspecs of packages to install. If this is not provided, the package is guessed from the command.
* `--channel <CHANNELS> (-c)`: The channel to install the packages from. If not specified the default channel is used.
* `--force-reinstall` If specified a new environment is always created even if one already exists.

```shell
pixi exec python

# Add a constraint to the python version
pixi exec -s python=3.9 python

# Run ipython and include the py-rattler package in the environment
pixi exec -s ipython -s py-rattler ipython

# Force reinstall to recreate the environment and get the latest package versions
pixi exec --force-reinstall -s ipython -s py-rattler ipython
```

## `remove`

Removes dependencies from the [manifest file](project_configuration.md).

If the project manifest is a `pyproject.toml`, removing a pypi dependency with the `--pypi` flag will remove it from either
- the native pyproject `project.dependencies` array or the native `project.optional-dependencies` table (if a feature is specified)
- pixi `pypi-dependencies` tables of the default or a named feature (if a feature is specified)

##### Arguments

1. `<DEPS>...`: List of dependencies you wish to remove from the project.

##### Options

- `--manifest-path <MANIFEST_PATH>`: the path to [manifest file](project_configuration.md), by default it searches for one in the parent directories.
- `--host`: Specifies a host dependency, important for building a package.
- `--build`: Specifies a build dependency, important for building a package.
- `--pypi`: Specifies a PyPI dependency, not a conda package.
- `--platform <PLATFORM> (-p)`: The platform from which the dependency should be removed.
- `--feature <FEATURE> (-f)`: The feature from which the dependency should be removed.
- `--no-install`: Don't install the environment, only remove the package from the lock-file and manifest.
- `--no-lockfile-update`: Don't update the lock-file, implies the `--no-install` flag.

```shell
pixi remove numpy
pixi remove numpy pandas pytorch
pixi remove --manifest-path ~/myproject/pixi.toml numpy
pixi remove --host python
pixi remove --build cmake
pixi remove --pypi requests
pixi remove --platform osx-64 --build clang
pixi remove --feature featurex clang
pixi remove --feature featurex --platform osx-64 clang
pixi remove --feature featurex --platform osx-64 --build clang
pixi remove --no-install numpy
```

## `task`

If you want to make a shorthand for a specific command you can add a task for it.

##### Options

- `--manifest-path <MANIFEST_PATH>`: the path to [manifest file](project_configuration.md), by default it searches for one in the parent directories.

### `task add`

Add a task to the [manifest file](project_configuration.md), use `--depends-on` to add tasks you want to run before this task, e.g. build before an execute task.

##### Arguments

1. `<NAME>`: The name of the task.
2. `<COMMAND>`: The command to run. This can be more than one word.
!!! info
    If you are using `$` for env variables they will be resolved before adding them to the task.
    If you want to use `$` in the task you need to escape it with a `\`, e.g. `echo \$HOME`.

##### Options

- `--platform <PLATFORM> (-p)`: the platform for which this task should be added.
- `--feature <FEATURE> (-f)`: the feature for which the task is added, if non provided the default tasks will be added.
- `--depends-on <DEPENDS_ON>`: the task it depends on to be run before the one your adding.
- `--cwd <CWD>`: the working directory for the task relative to the root of the project.
- `--env <ENV>`: the environment variables as `key=value` pairs for the task, can be used multiple times, e.g. `--env "VAR1=VALUE1" --env "VAR2=VALUE2"`.
- `--description <DESCRIPTION>`: a description of the task.

```shell
pixi task add cow cowpy "Hello User"
pixi task add tls ls --cwd tests
pixi task add test cargo t --depends-on build
pixi task add build-osx "METAL=1 cargo build" --platform osx-64
pixi task add train python train.py --feature cuda
pixi task add publish-pypi "hatch publish --yes --repo main" --feature build --env HATCH_CONFIG=config/hatch.toml --description "Publish the package to pypi"
```

This adds the following to the [manifest file](project_configuration.md):

```toml
[tasks]
cow = "cowpy \"Hello User\""
tls = { cmd = "ls", cwd = "tests" }
test = { cmd = "cargo t", depends-on = ["build"] }

[target.osx-64.tasks]
build-osx = "METAL=1 cargo build"

[feature.cuda.tasks]
train = "python train.py"

[feature.build.tasks]
publish-pypi = { cmd = "hatch publish --yes --repo main", env = { HATCH_CONFIG = "config/hatch.toml" }, description = "Publish the package to pypi" }
```

Which you can then run with the `run` command:

```shell
pixi run cow
# Extra arguments will be passed to the tasks command.
pixi run test --test test1
```

### `task remove`

Remove the task from the [manifest file](project_configuration.md)

##### Arguments

- `<NAMES>`: The names of the tasks, space separated.

##### Options

- `--platform <PLATFORM> (-p)`: the platform for which this task is removed.
- `--feature <FEATURE> (-f)`: the feature for which the task is removed.

```shell
pixi task remove cow
pixi task remove --platform linux-64 test
pixi task remove --feature cuda task
```

### `task alias`

Create an alias for a task.

##### Arguments

1. `<ALIAS>`: The alias name
2. `<DEPENDS_ON>`: The names of the tasks you want to execute on this alias, order counts, first one runs first.

##### Options

- `--platform <PLATFORM> (-p)`: the platform for which this alias is created.

```shell
pixi task alias test-all test-py test-cpp test-rust
pixi task alias --platform linux-64 test test-linux
pixi task alias moo cow
```

### `task list`

List all tasks in the project.

##### Options

- `--environment`(`-e`): the environment's tasks list, if non is provided the default tasks will be listed.
- `--summary`(`-s`): list the tasks per environment.

```shell
pixi task list
pixi task list --environment cuda
pixi task list --summary
```

## `list`

List project's packages. Highlighted packages are explicit dependencies.

##### Arguments

1. `[REGEX]`: List only packages matching a regular expression (optional).

##### Options

- `--platform <PLATFORM> (-p)`: The platform to list packages for. Defaults to the current platform
- `--json`: Whether to output in json format.
- `--json-pretty`: Whether to output in pretty json format
- `--sort-by <SORT_BY>`: Sorting strategy [default: name] [possible values: size, name, type]
- `--explicit (-x)`: Only list the packages that are explicitly added to the [manifest file](project_configuration.md).
- `--manifest-path <MANIFEST_PATH>`: The path to [manifest file](project_configuration.md), by default it searches for one in the parent directories.
- `--environment (-e)`: The environment's packages to list, if non is provided the default environment's packages will be listed.
- `--frozen`: install the environment as defined in the lock file, doesn't update `pixi.lock` if it isn't up-to-date with [manifest file](project_configuration.md). It can also be controlled by the `PIXI_FROZEN` environment variable (example: `PIXI_FROZEN=true`).
- `--locked`: Only install if the `pixi.lock` is up-to-date with the [manifest file](project_configuration.md)[^1]. It can also be controlled by the `PIXI_LOCKED` environment variable (example: `PIXI_LOCKED=true`). Conflicts with `--frozen`.
- `--no-install`: Don't install the environment for pypi solving, only update the lock-file if it can solve without installing. (Implied by `--frozen` and `--locked`)

```shell
pixi list
pixi list py
pixi list --json-pretty
pixi list --explicit
pixi list --sort-by size
pixi list --platform win-64
pixi list --environment cuda
pixi list --frozen
pixi list --locked
pixi list --no-install
```

Output will look like this, where `python` will be green as it is the package that was explicitly added to the [manifest file](project_configuration.md):

```shell
➜ pixi list
 Package           Version     Build               Size       Kind   Source
 _libgcc_mutex     0.1         conda_forge         2.5 KiB    conda  _libgcc_mutex-0.1-conda_forge.tar.bz2
 _openmp_mutex     4.5         2_gnu               23.1 KiB   conda  _openmp_mutex-4.5-2_gnu.tar.bz2
 bzip2             1.0.8       hd590300_5          248.3 KiB  conda  bzip2-1.0.8-hd590300_5.conda
 ca-certificates   2023.11.17  hbcca054_0          150.5 KiB  conda  ca-certificates-2023.11.17-hbcca054_0.conda
 ld_impl_linux-64  2.40        h41732ed_0          688.2 KiB  conda  ld_impl_linux-64-2.40-h41732ed_0.conda
 libexpat          2.5.0       hcb278e6_1          76.2 KiB   conda  libexpat-2.5.0-hcb278e6_1.conda
 libffi            3.4.2       h7f98852_5          56.9 KiB   conda  libffi-3.4.2-h7f98852_5.tar.bz2
 libgcc-ng         13.2.0      h807b86a_4          755.7 KiB  conda  libgcc-ng-13.2.0-h807b86a_4.conda
 libgomp           13.2.0      h807b86a_4          412.2 KiB  conda  libgomp-13.2.0-h807b86a_4.conda
 libnsl            2.0.1       hd590300_0          32.6 KiB   conda  libnsl-2.0.1-hd590300_0.conda
 libsqlite         3.44.2      h2797004_0          826 KiB    conda  libsqlite-3.44.2-h2797004_0.conda
 libuuid           2.38.1      h0b41bf4_0          32.8 KiB   conda  libuuid-2.38.1-h0b41bf4_0.conda
 libxcrypt         4.4.36      hd590300_1          98 KiB     conda  libxcrypt-4.4.36-hd590300_1.conda
 libzlib           1.2.13      hd590300_5          60.1 KiB   conda  libzlib-1.2.13-hd590300_5.conda
 ncurses           6.4         h59595ed_2          863.7 KiB  conda  ncurses-6.4-h59595ed_2.conda
 openssl           3.2.0       hd590300_1          2.7 MiB    conda  openssl-3.2.0-hd590300_1.conda
 python            3.12.1      hab00c5b_1_cpython  30.8 MiB   conda  python-3.12.1-hab00c5b_1_cpython.conda
 readline          8.2         h8228510_1          274.9 KiB  conda  readline-8.2-h8228510_1.conda
 tk                8.6.13      noxft_h4845f30_101  3.2 MiB    conda  tk-8.6.13-noxft_h4845f30_101.conda
 tzdata            2023d       h0c530f3_0          116.8 KiB  conda  tzdata-2023d-h0c530f3_0.conda
 xz                5.2.6       h166bdaf_0          408.6 KiB  conda  xz-5.2.6-h166bdaf_0.tar.bz2
```

## `tree`

Display the project's packages in a tree. Highlighted packages are those specified in the manifest.

The package tree can also be inverted (`-i`), to see which packages require a specific dependencies.

##### Arguments

- `REGEX` optional regex of which dependencies to filter the tree to, or which dependencies to start with when inverting the tree.

##### Options

- `--invert (-i)`: Invert the dependency tree, that is given a `REGEX` pattern that matches some packages, show all the packages that depend on those.
- `--platform <PLATFORM> (-p)`: The platform to list packages for. Defaults to the current platform
- `--manifest-path <MANIFEST_PATH>`: The path to [manifest file](project_configuration.md), by default it searches for one in the parent directories.
- `--environment (-e)`: The environment's packages to list, if non is provided the default environment's packages will be listed.
- `--frozen`: install the environment as defined in the lock file, doesn't update `pixi.lock` if it isn't up-to-date with [manifest file](project_configuration.md). It can also be controlled by the `PIXI_FROZEN` environment variable (example: `PIXI_FROZEN=true`).
- `--locked`: Only install if the `pixi.lock` is up-to-date with the [manifest file](project_configuration.md)[^1]. It can also be controlled by the `PIXI_LOCKED` environment variable (example: `PIXI_LOCKED=true`). Conflicts with `--frozen`.
- `--no-install`: Don't install the environment for pypi solving, only update the lock-file if it can solve without installing. (Implied by `--frozen` and `--locked`)

```shell
pixi tree
pixi tree pre-commit
pixi tree -i yaml
pixi tree --environment docs
pixi tree --platform win-64
```

!!! warning
    Use `-v` to show which `pypi` packages are not yet parsed correctly. The `extras` and `markers` parsing is still under development.

Output will look like this, where direct packages in the [manifest file](project_configuration.md) will be green.
Once a package has been displayed once, the tree won't continue to recurse through its dependencies (compare the first time `python` appears, vs the rest), and it will instead be marked with a star `(*)`.

Version numbers are colored by the package type, yellow for Conda packages and blue for PyPI.

```shell
➜ pixi tree
├── pre-commit v3.3.3
│   ├── cfgv v3.3.1
│   │   └── python v3.12.2
│   │       ├── bzip2 v1.0.8
│   │       ├── libexpat v2.6.2
│   │       ├── libffi v3.4.2
│   │       ├── libsqlite v3.45.2
│   │       │   └── libzlib v1.2.13
│   │       ├── libzlib v1.2.13 (*)
│   │       ├── ncurses v6.4.20240210
│   │       ├── openssl v3.2.1
│   │       ├── readline v8.2
│   │       │   └── ncurses v6.4.20240210 (*)
│   │       ├── tk v8.6.13
│   │       │   └── libzlib v1.2.13 (*)
│   │       └── xz v5.2.6
│   ├── identify v2.5.35
│   │   └── python v3.12.2 (*)
...
└── tbump v6.9.0
...
    └── tomlkit v0.12.4
        └── python v3.12.2 (*)
```

A regex pattern can be specified to filter the tree to just those that show a specific direct, or transitive dependency:

```shell
➜ pixi tree pre-commit
└── pre-commit v3.3.3
    ├── virtualenv v20.25.1
    │   ├── filelock v3.13.1
    │   │   └── python v3.12.2
    │   │       ├── libexpat v2.6.2
    │   │       ├── readline v8.2
    │   │       │   └── ncurses v6.4.20240210
    │   │       ├── libsqlite v3.45.2
    │   │       │   └── libzlib v1.2.13
    │   │       ├── bzip2 v1.0.8
    │   │       ├── libzlib v1.2.13 (*)
    │   │       ├── libffi v3.4.2
    │   │       ├── tk v8.6.13
    │   │       │   └── libzlib v1.2.13 (*)
    │   │       ├── xz v5.2.6
    │   │       ├── ncurses v6.4.20240210 (*)
    │   │       └── openssl v3.2.1
    │   ├── platformdirs v4.2.0
    │   │   └── python v3.12.2 (*)
    │   ├── distlib v0.3.8
    │   │   └── python v3.12.2 (*)
    │   └── python v3.12.2 (*)
    ├── pyyaml v6.0.1
...
```

Additionally, the tree can be inverted, and it can show which packages depend on a regex pattern.
The packages specified in the manifest will also be highlighted (in this case `cffconvert` and `pre-commit` would be).

```shell
➜ pixi tree -i yaml

ruamel.yaml v0.18.6
├── pykwalify v1.8.0
│   └── cffconvert v2.0.0
└── cffconvert v2.0.0

pyyaml v6.0.1
└── pre-commit v3.3.3

ruamel.yaml.clib v0.2.8
└── ruamel.yaml v0.18.6
    ├── pykwalify v1.8.0
    │   └── cffconvert v2.0.0
    └── cffconvert v2.0.0

yaml v0.2.5
└── pyyaml v6.0.1
    └── pre-commit v3.3.3
```

## `shell`

This command starts a new shell in the project's environment.
To exit the pixi shell, simply run `exit`.

##### Options

- `--change-ps1 <true or false>`: When set to false, the `(pixi)` prefix in the shell prompt is removed (default: `true`). The default behavior can be [configured globally](pixi_configuration.md#change-ps1).
- `--manifest-path <MANIFEST_PATH>`: the path to [manifest file](project_configuration.md), by default it searches for one in the parent directories.
- `--frozen`: install the environment as defined in the lock file, doesn't update `pixi.lock` if it isn't up-to-date with [manifest file](project_configuration.md). It can also be controlled by the `PIXI_FROZEN` environment variable (example: `PIXI_FROZEN=true`).
- `--locked`: only install if the `pixi.lock` is up-to-date with the [manifest file](project_configuration.md)[^1]. It can also be controlled by the `PIXI_LOCKED` environment variable (example: `PIXI_LOCKED=true`). Conflicts with `--frozen`.
- `--environment <ENVIRONMENT> (-e)`: The environment to activate the shell in, if none are provided the default environment will be used or a selector will be given to select the right environment.

```shell
pixi shell
exit
pixi shell --manifest-path ~/myproject/pixi.toml
exit
pixi shell --frozen
exit
pixi shell --locked
exit
pixi shell --environment cuda
exit
```

## `shell-hook`

This command prints the activation script of an environment.

##### Options

- `--shell <SHELL> (-s)`: The shell for which the activation script should be printed. Defaults to the current shell.
  Currently supported variants: [`bash`, `zsh`, `xonsh`, `cmd`, `powershell`, `fish`, `nushell`]
- `--manifest-path`: the path to [manifest file](project_configuration.md), by default it searches for one in the parent directories.
- `--frozen`: install the environment as defined in the lock file, doesn't update `pixi.lock` if it isn't up-to-date with [manifest file](project_configuration.md). It can also be controlled by the `PIXI_FROZEN` environment variable (example: `PIXI_FROZEN=true`).
- `--locked`: only install if the `pixi.lock` is up-to-date with the [manifest file](project_configuration.md)[^1]. It can also be controlled by the `PIXI_LOCKED` environment variable (example: `PIXI_LOCKED=true`). Conflicts with `--frozen`.
- `--environment <ENVIRONMENT> (-e)`: The environment to activate, if none are provided the default environment will be used or a selector will be given to select the right environment.
- `--json`: Print all environment variables that are exported by running the activation script as JSON. When specifying
  this option, `--shell` is ignored.

```shell
pixi shell-hook
pixi shell-hook --shell bash
pixi shell-hook --shell zsh
pixi shell-hook -s powershell
pixi shell-hook --manifest-path ~/myproject/pixi.toml
pixi shell-hook --frozen
pixi shell-hook --locked
pixi shell-hook --environment cuda
pixi shell-hook --json
```

Example use-case, when you want to get rid of the `pixi` executable in a Docker container.

```shell
pixi shell-hook --shell bash > /etc/profile.d/pixi.sh
rm ~/.pixi/bin/pixi # Now the environment will be activated without the need for the pixi executable.
```

## `search`

Search a package, output will list the latest version of the package.

##### Arguments

1. `<PACKAGE>`: Name of package to search, it's possible to use wildcards (`*`).

###### Options

- `--manifest-path <MANIFEST_PATH>`: the path to [manifest file](project_configuration.md), by default it searches for one in the parent directories.
- `--channel <CHANNEL> (-c)`: specify a channel that the project uses. Defaults to `conda-forge`. (Allowed to be used more than once)
- `--limit <LIMIT> (-l)`: optionally limit the number of search results
- `--platform <PLATFORM> (-p)`: specify a platform that you want to search for. (default: current platform)

```zsh
pixi search pixi
pixi search --limit 30 "py*"
# search in a different channel and for a specific platform
pixi search -c robostack --platform linux-64 "plotjuggler*"
```

## `self-update`

Update pixi to the latest version or a specific version. If pixi was installed using another package manager this feature might not
be available and pixi should be updated using the package manager used to install it.

##### Options

- `--version <VERSION>`: The desired version (to downgrade or upgrade to). Update to the latest version if not specified.

```shell
pixi self-update
pixi self-update --version 0.13.0
```

## `info`

Shows helpful information about the pixi installation, cache directories, disk usage, and more.
More information [here](../advanced/explain_info_command.md).

##### Options

- `--manifest-path <MANIFEST_PATH>`: the path to [manifest file](project_configuration.md), by default it searches for one in the parent directories.
- `--extended`: extend the information with more slow queries to the system, like directory sizes.
- `--json`: Get a machine-readable version of the information as output.

```shell
pixi info
pixi info --json --extended
```
## `clean`

Clean the parts of your system which are touched by pixi.
Defaults to cleaning the environments and task cache.
Use the `cache` subcommand to clean the cache

##### Options
- `--manifest-path <MANIFEST_PATH>`: the path to [manifest file](project_configuration.md), by default it searches for one in the parent directories.
- `--environment <ENVIRONMENT> (-e)`: The environment to clean, if none are provided all environments will be removed.

```shell
pixi clean
```

### `clean cache`

Clean the pixi cache on your system.

##### Options
- `--pypi`: Clean the pypi cache.
- `--conda`: Clean the conda cache.
- `--yes`: Skip the confirmation prompt.

```shell
pixi clean cache # clean all pixi caches
pixi clean cache --pypi # clean only the pypi cache
pixi clean cache --conda # clean only the conda cache
pixi clean cache --yes # skip the confirmation prompt
```

## `upload`

Upload a package to a prefix.dev channel

##### Arguments

1. `<HOST>`: The host + channel to upload to.
2. `<PACKAGE_FILE>`: The package file to upload.

```shell
pixi upload https://prefix.dev/api/v1/upload/my_channel my_package.conda
```

## `auth`

This command is used to authenticate the user's access to remote hosts such as `prefix.dev` or `anaconda.org` for private channels.

### `auth login`

Store authentication information for given host.

!!! tip
    The host is real hostname not a channel.

##### Arguments

1. `<HOST>`: The host to authenticate with.

##### Options

- `--token <TOKEN>`: The token to use for authentication with prefix.dev.
- `--username <USERNAME>`: The username to use for basic HTTP authentication
- `--password <PASSWORD>`: The password to use for basic HTTP authentication.
- `--conda-token <CONDA_TOKEN>`: The token to use on `anaconda.org` / `quetz` authentication.

```shell
pixi auth login repo.prefix.dev --token pfx_JQEV-m_2bdz-D8NSyRSaAndHANx0qHjq7f2iD
pixi auth login anaconda.org --conda-token ABCDEFGHIJKLMNOP
pixi auth login https://myquetz.server --username john --password xxxxxx
```

### `auth logout`

Remove authentication information for a given host.

##### Arguments

1. `<HOST>`: The host to authenticate with.

```shell
pixi auth logout <HOST>
pixi auth logout repo.prefix.dev
pixi auth logout anaconda.org
```

## `config`

Use this command to manage the configuration.

##### Options

- `--system (-s)`: Specify management scope to system configuration.
- `--global (-g)`: Specify management scope to global configuration.
- `--local (-l)`: Specify management scope to local configuration.

Checkout the [pixi configuration](./pixi_configuration.md) for more information about the locations.

### `config edit`

Edit the configuration file in the default editor.


##### Arguments

1. `[EDITOR]`: The editor to use, defaults to `EDITOR` environment variable or `nano` on Unix and `notepad` on Windows

```shell
pixi config edit --system
pixi config edit --local
pixi config edit -g
pixi config edit --global code
pixi config edit --system vim
```

### `config list`

List the configuration

##### Arguments

1. `[KEY]`: The key to list the value of. (all if not provided)

##### Options

- `--json`: Output the configuration in JSON format.

```shell
pixi config list default-channels
pixi config list --json
pixi config list --system
pixi config list -g
```

### `config prepend`

Prepend a value to a list configuration key.

##### Arguments

1. `<KEY>`: The key to prepend the value to.
2. `<VALUE>`: The value to prepend.

```shell
pixi config prepend default-channels conda-forge
```

### `config append`

Append a value to a list configuration key.

##### Arguments

1. `<KEY>`: The key to append the value to.
2. `<VALUE>`: The value to append.

```shell
pixi config append default-channels robostack
pixi config append default-channels bioconda --global
```

### `config set`

Set a configuration key to a value.

##### Arguments

1. `<KEY>`: The key to set the value of.
2. `[VALUE]`: The value to set. (if not provided, the key will be removed)

```shell
pixi config set default-channels '["conda-forge", "bioconda"]'
pixi config set --global mirrors '{"https://conda.anaconda.org/": ["https://prefix.dev/conda-forge"]}'
pixi config set repodata-config.disable-zstd true --system
pixi config set --global detached-environments "/opt/pixi/envs"
pixi config set detached-environments false
```

### `config unset`

Unset a configuration key.

##### Arguments

1. `<KEY>`: The key to unset.

```shell
pixi config unset default-channels
pixi config unset --global mirrors
pixi config unset repodata-config.disable-zstd --system
```

## `global`

Global is the main entry point for the part of pixi that executes on the global(system) level.
All commands in this section are used to manage global installations of packages and environments through the global manifest.
More info on the global manifest can be found [here](../features/global_tools.md).

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

- `--manifest-path <MANIFEST_PATH>`: the path to [manifest file](project_configuration.md), by default it searches for one in the parent directories.

### `project channel add`

Add channels to the channel list in the project configuration.
When you add channels, the channels are tested for existence, added to the lock file and the environment is reinstalled.

##### Arguments

1. `<CHANNEL>`: The channels to add, name or URL.

##### Options

- `--no-install`: do not update the environment, only add changed packages to the lock-file.
- `--feature <FEATURE> (-f)`: The feature for which the channel is added.

```
pixi project channel add robostack
pixi project channel add bioconda conda-forge robostack
pixi project channel add file:///home/user/local_channel
pixi project channel add https://repo.prefix.dev/conda-forge
pixi project channel add --no-install robostack
pixi project channel add --feature cuda nvidia
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

### `project export conda_environment`

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

### `project export conda_explicit_spec`

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
pixi project export conda_explicit_spec output
pixi project export conda_explicit_spec -e default -e test -p linux-64 output
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

[^1]:
    An **up-to-date** lock file means that the dependencies in the lock file are allowed by the dependencies in the manifest file.
    For example

    - a manifest with `python = ">= 3.11"` is up-to-date with a `name: python, version: 3.11.0` in the `pixi.lock`.
    - a manifest with `python = ">= 3.12"` is **not** up-to-date with a `name: python, version: 3.11.0` in the `pixi.lock`.

    Being up-to-date does **not** mean that the lock file holds the latest version available on the channel for the given dependency.
