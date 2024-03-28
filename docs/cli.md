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
- `--color <COLOR>`: Whether the log needs to be colored [env: `PIXI_COLOR=`] [default: `auto`] [possible values: always, never, auto].
Pixi also honor the `FORCE_COLOR` and `NO_COLOR` environment variables.
They both take precedence over `--color` and `PIXI_COLOR`.


## `init`

This command is used to create a new project.
It initializes a `pixi.toml` file and also prepares a `.gitignore` to prevent the environment from being added to `git`.

It also supports the [`pyproject.toml`](./advanced/pyproject_toml.md) file, if you have a `pyproject.toml` file in the directory where you run `pixi init`, it appends the pixi data to the `pyproject.toml` instead of a new `pixi.toml` file.


##### Arguments

1. `[PATH]`: Where to place the project (defaults to current path) [default: `.`]

##### Options

- `--channel <CHANNEL> (-c)`: specify a channel that the project uses. Defaults to `conda-forge`. (Allowed to be used more than once)
- `--platform <PLATFORM> (-p)`: specify a platform that the project supports. (Allowed to be used more than once)
- `--import <ENV_FILE> (-i)`: Import an existing conda environment file, e.g. `environment.yml`.
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
```

## `add`

Adds dependencies to the [manifest file](configuration.md).
It will only add if the package with its version constraint is able to work with rest of the dependencies in the project.
[More info](advanced/multi_platform_configuration.md) on multi-platform configuration.

##### Arguments

1. `<SPECS>`: The package(s) to add, space separated. The version constraint is optional.

##### Options

- `--manifest-path <MANIFEST_PATH>`: the path to [manifest file](configuration.md), by default it searches for one in the parent directories. It can also be controlled by the `PIXI_PROJECT_MANIFEST` environment variable which allows the default to be overridden by `pixi shell`.
- `--host`: Specifies a host dependency, important for building a package.
- `--build`: Specifies a build dependency, important for building a package.
- `--pypi`: Specifies a PyPI dependency, not a conda package.
    Parses dependencies as [PEP508](https://peps.python.org/pep-0508/) requirements, supporting extras and versions.
    See [configuration](configuration.md) for details.
- `--no-install`: Don't install the package to the environment, only add the package to the lock-file.
- `--no-lockfile-update`: Don't update the lock-file, implies the `--no-install` flag.
- `--platform <PLATFORM> (-p)`: The platform for which the dependency should be added. (Allowed to be used more than once)
- `--feature <FEATURE> (-f)`: The feature for which the dependency should be added.

```shell
pixi add numpy
pixi add numpy pandas "pytorch>=1.8"
pixi add "numpy>=1.22,<1.24"
pixi add --manifest-path ~/myproject/pixi.toml numpy
pixi add --host "python>=3.9.0"
pixi add --build cmake
pixi add --pypi requests[security]
pixi add --platform osx-64 --build clang
pixi add --no-install numpy
pixi add --no-lockfile-update numpy
pixi add --feature featurex numpy
```

## `install`

Installs all dependencies specified in the lockfile `pixi.lock`.
Which gets generated on `pixi add` or when you manually change the [manifest file](configuration.md) file and run `pixi install`.

##### Options
- `--manifest-path <MANIFEST_PATH>`: the path to [manifest file](configuration.md), by default it searches for one in the parent directories.
- `--environment`(`-e`): The environment's packages to list, if non is provided the default environment's packages will be listed. It can also be controlled by the `PIXI_ENVIRONMENT_NAME` environment variable which allows the default to be overridden by `pixi shell`.
- `--frozen`: install the environment as defined in the lockfile. Without checking the status of the lockfile. It can also be controlled by the `PIXI_FROZEN` environment variable (example: `PIXI_FROZEN=true`).
- `--locked`: only install if the `pixi.lock` is up-to-date with the [manifest file](configuration.md)[^1]. It can also be controlled by the `PIXI_LOCKED` environment variable (example: `PIXI_LOCKED=true`). Conflicts with `--frozen`.

```shell
pixi install
pixi install --manifest-path ~/myproject/pixi.toml
pixi install --frozen
pixi install --locked
```

## `run`

The `run` commands first checks if the environment is ready to use.
When you didn't run `pixi install` the run command will do that for you.
The custom tasks defined in the [manifest file](configuration.md) are also available through the run command.

You cannot run `pixi run source setup.bash` as `source` is not available in the `deno_task_shell` commandos and not an executable.

##### Arguments

1. `[TASK]...`  The task you want to run in the projects environment, this can also be a normal command. And all arguments after the task will be passed to the task.

##### Options

- `--manifest-path <MANIFEST_PATH>`: the path to [manifest file](configuration.md), by default it searches for one in the parent directories. It can also be controlled by the `PIXI_PROJECT_MANIFEST` environment variable which allows the default to be overridden by `pixi shell`.
- `--environment`(`-e`): The environment's packages to list, if non is provided the default environment's packages will be listed. It can also be controlled by the `PIXI_ENVIRONMENT_NAME` environment variable which allows the default to be overridden by `pixi shell`.
- `--frozen`: install the environment as defined in the lockfile. Without checking the status of the lockfile. It can also be controlled by the `PIXI_FROZEN` environment variable (example: `PIXI_FROZEN=true`).
- `--locked`: only install if the `pixi.lock` is up-to-date with the [manifest file](configuration.md)[^1]. It can also be controlled by the `PIXI_LOCKED` environment variable (example: `PIXI_LOCKED=true`). Conflicts with `--frozen`.
- `--environment <ENVIRONMENT> (-e)`: The environment to run the task in, if none are provided the default environment will be used or a selector will be given to select the right environment.

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
```

!!! info
      In `pixi` the [`deno_task_shell`](https://deno.land/manual@v1.35.0/tools/task_runner#task-runner) is the underlying runner of the run command.
      Checkout their [documentation](https://deno.land/manual@v1.35.0/tools/task_runner#task-runner) for the syntax and available commands.
      This is done so that the run commands can be run across all platforms.

!!! tip "Cross environment tasks"
    If you're using the `depends_on` feature of the `tasks`, the tasks will be run in the order you specified them.
    The `depends_on` can be used cross environment, e.g. you have this `pixi.toml`:
    ??? "pixi.toml"
        ```toml
        [tasks]
        start = { cmd = "python start.py", depends_on = ["build"] }

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


## `remove`

Removes dependencies from the [manifest file](configuration.md).

##### Arguments

1. `<DEPS>...`: List of dependencies you wish to remove from the project.

##### Options

- `--manifest-path <MANIFEST_PATH>`: the path to [manifest file](configuration.md), by default it searches for one in the parent directories. It can also be controlled by the `PIXI_PROJECT_MANIFEST` environment variable which allows the default to be overridden by `pixi shell`.
- `--host`: Specifies a host dependency, important for building a package.
- `--build`: Specifies a build dependency, important for building a package.
- `--pypi`: Specifies a PyPI dependency, not a conda package.
- `--platform <PLATFORM> (-p)`: The platform from which the dependency should be removed.
- `--feature <FEATURE> (-f)`: The feature from which the dependency should be removed.

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
```

## `task`

If you want to make a shorthand for a specific command you can add a task for it.

##### Options

- `--manifest-path <MANIFEST_PATH>`: the path to [manifest file](configuration.md), by default it searches for one in the parent directories. It can also be controlled by the `PIXI_PROJECT_MANIFEST` environment variable which allows the default to be overridden by `pixi shell`.
- `--environment`(`-e`): The environment's packages to list, if non is provided the default environment's packages will be listed. It can also be controlled by the `PIXI_ENVIRONMENT_NAME` environment variable which allows the default to be overridden by `pixi shell`.

### `task add`

Add a task to the [manifest file](configuration.md), use `--depends-on` to add tasks you want to run before this task, e.g. build before an execute task.

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

```shell
pixi task add cow cowpy "Hello User"
pixi task add tls ls --cwd tests
pixi task add test cargo t --depends-on build
pixi task add build-osx "METAL=1 cargo build" --platform osx-64
pixi task add train python train.py --feature cuda
```

This adds the following to the [manifest file](configuration.md):

```toml
[tasks]
cow = "cowpy \"Hello User\""
tls = { cmd = "ls", cwd = "tests" }
test = { cmd = "cargo t", depends_on = ["build"] }

[target.osx-64.tasks]
build-osx = "METAL=1 cargo build"

[feature.cuda.tasks]
train = "python train.py"
```

Which you can then run with the `run` command:

```shell
pixi run cow
# Extra arguments will be passed to the tasks command.
pixi run test --test test1
```

### `task remove`

Remove the task from the [manifest file](configuration.md)

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
- `--summary`(`-s`): the output gets formatted to be machine parsable. (Used in the autocompletion of `pixi run`).

```shell
pixi task list
pixi task list --environment cuda
pixi task list --summary
```

## `list`

List project's packages. Highlighted packages are explicit dependencies.

##### Options

- `--platform <PLATFORM> (-p)`: The platform to list packages for. Defaults to the current platform
- `--json`: Whether to output in json format.
- `--json-pretty`: Whether to output in pretty json format
- `--sort-by <SORT_BY>`: Sorting strategy [default: name] [possible values: size, name, type]
- `--manifest-path <MANIFEST_PATH>`: The path to [manifest file](configuration.md), by default it searches for one in the parent directories. It can also be controlled by the `PIXI_PROJECT_MANIFEST` environment variable which allows the default to be overridden by `pixi shell`.
- `--environment`(`-e`): The environment's packages to list, if non is provided the default environment's packages will be listed. It can also be controlled by the `PIXI_ENVIRONMENT_NAME` environment variable which allows the default to be overridden by `pixi shell`.
- `--frozen`: Install the environment as defined in the lockfile. Without checking the status of the lockfile. It can also be controlled by the `PIXI_FROZEN` environment variable (example: `PIXI_FROZEN=true`).
- `--locked`: Only install if the `pixi.lock` is up-to-date with the [manifest file](configuration.md)[^1]. It can also be controlled by the `PIXI_LOCKED` environment variable (example: `PIXI_LOCKED=true`). Conflicts with `--frozen`.
- `--no-install`: Don't install the environment for pypi solving, only update the lock-file if it can solve without installing. (Implied by `--frozen` and `--locked`)

```shell

```shell
pixi list
pixi list --json-pretty
pixi list --sort-by size
pixi list --platform win-64
pixi list --environment cuda
pixi list --frozen
pixi list --locked
pixi list --no-install
```
Output will look like this, where `python` will be green as it is the package that was explicitly added to the [manifest file](configuration.md):

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

## `shell`

This command starts a new shell in the project's environment.
To exit the pixi shell, simply run `exit`.

The command sets the `PIXI_PROJECT_MANIFEST` environment variable which most of the other commands use as a default for `--manifest-path` so that they work with the shell's manifest. The `PIXI_ENVIRONMENT_NAME` environment variable is also set which will cause commands using `--environment`(`-e`) to use it as a default as well.

##### Options

- `--manifest-path <MANIFEST_PATH>`: the path to [manifest file](configuration.md), by default it searches for one in the parent directories. It can also be controlled by the `PIXI_PROJECT_MANIFEST` environment variable which allows the default to be overridden by `pixi shell`.
- `--frozen`: install the environment as defined in the lockfile. Without checking the status of the lockfile. It can also be controlled by the `PIXI_FROZEN` environment variable (example: `PIXI_FROZEN=true`).
- `--locked`: only install if the `pixi.lock` is up-to-date with the [manifest file](configuration.md)[^1]. It can also be controlled by the `PIXI_LOCKED` environment variable (example: `PIXI_LOCKED=true`). Conflicts with `--frozen`.
- `--environment`(`-e`): The environment's packages to list, if non is provided the default environment's packages will be listed. It can also be controlled by the `PIXI_ENVIRONMENT_NAME` environment variable which allows the default to be overridden by `pixi shell`.

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
    Currently supported variants: [`bash`,  `zsh`,  `xonsh`,  `cmd`,  `powershell`,  `fish`,  `nushell`]
- `--manifest-path`: the path to [manifest file](configuration.md), by default it searches for one in the parent directories. It can also be controlled by the `PIXI_PROJECT_MANIFEST` environment variable which allows the default to be overridden by `pixi shell`.
- `--frozen`: install the environment as defined in the lockfile. Without checking the status of the lockfile. It can also be controlled by the `PIXI_FROZEN` environment variable (example: `PIXI_FROZEN=true`).
- `--locked`: only install if the `pixi.lock` is up-to-date with the [manifest file](configuration.md)[^1]. It can also be controlled by the `PIXI_LOCKED` environment variable (example: `PIXI_LOCKED=true`). Conflicts with `--frozen`.
- `--environment`(`-e`): The environment's packages to list, if non is provided the default environment's packages will be listed. It can also be controlled by the `PIXI_ENVIRONMENT_NAME` environment variable which allows the default to be overridden by `pixi shell`.

```shell
pixi shell-hook
pixi shell-hook --shell bash
pixi shell-hook --shell zsh
pixi shell-hook -s powershell
pixi shell-hook --manifest-path ~/myproject/pixi.toml
pixi shell-hook --frozen
pixi shell-hook --locked
pixi shell-hook --environment cuda
```
Example use-case, when you want to get rid of the `pixi` executable in a Docker container.
```shell
pixi shell-hook --shell bash > /etc/profile.d/pixi.sh
rm ~/.pixi/bin/pixi # Now the environment will be activated without the need for the pixi executable.
```

## `search`

Search a package, output will list the latest version of the package.

##### Arguments
1. `<PACKAGE>`:  Name of package to search, it's possible to use wildcards (`*`).


###### Options

- `--manifest-path <MANIFEST_PATH>`: the path to [manifest file](configuration.md), by default it searches for one in the parent directories. It can also be controlled by the `PIXI_PROJECT_MANIFEST` environment variable which allows the default to be overridden by `pixi shell`.
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

Update pixi to the latest version or a specific version. If the pixi binary is not found in the default location (e.g.
`~/.pixi/bin/pixi`), pixi won't update to prevent breaking the current installation (Homebrew, etc.). The behaviour can be
overridden with the `--force` flag

##### Options

- `--version <VERSION>`: The desired version (to downgrade or upgrade to). Update to the latest version if not specified.
- `--force`: Force the update even if the pixi binary is not found in the default location.

```shell
pixi self-update
pixi self-update --version 0.13.0
pixi self-update --force
```

## `info`

Shows helpful information about the pixi installation, cache directories, disk usage, and more.
More information [here](advanced/explain_info_command.md).

##### Options

- `--manifest-path <MANIFEST_PATH>`: the path to [manifest file](configuration.md), by default it searches for one in the parent directories. It can also be controlled by the `PIXI_PROJECT_MANIFEST` environment variable which allows the default to be overridden by `pixi shell`.
- `--extended`: extend the information with more slow queries to the system, like directory sizes.
- `--json`: Get a machine-readable version of the information as output.

```shell
pixi info
pixi info --json --extended
```

## `upload`

Upload a package to a prefix.dev channel

##### Arguments

1. `<HOST>`: The host + channel to upload to.
2. `<PACKAGE_FILE>`: The package file to upload.

```shell
pixi upload repo.prefix.dev/my_channel my_package.conda
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
pixi auth login repo.prefix.dev --token pfx_JQEV-m_2bdz-D8NSyRSaNdHANx0qHjq7f2iD
pixi auth login anaconda.org --conda-token ABCDEFGHIJKLMNOP
pixi auth login https://myquetz.server --user john --password xxxxxx
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

## `global`

Global is the main entry point for the part of pixi that executes on the
global(system) level.

!!! tip
    Binaries and environments installed globally are stored in `~/.pixi`
    by default, this can be changed by setting the `PIXI_HOME` environment
    variable.

### `global install`

This command installs package(s) into its own environment and adds the binary to `PATH`, allowing you to access it anywhere on your system without activating the environment.

##### Arguments

1.`<PACKAGE>`: The package(s) to install, this can also be a version constraint.

##### Options

- `--channel <CHANNEL> (-c)`: specify a channel that the project uses. Defaults to `conda-forge`. (Allowed to be used more than once)

```shell
pixi global install ruff
# multiple packages can be installed at once
pixi global install starship rattler-build
# specify the channel(s)
pixi global install --channel conda-forge --channel bioconda trackplot
# Or in a more concise form
pixi global install -c conda-forge -c bioconda trackplot

# Support full conda matchspec
pixi global install python=3.9.*
pixi global install "python [version='3.11.0', build_number=1]"
pixi global install "python [version='3.11.0', build=he550d4f_1_cpython]"
pixi global install python=3.11.0=h10a6764_1_cpython
```

After using global install, you can use the package you installed anywhere on your system.

### `global list`

This command shows the current installed global environments including what binaries come with it.
A global installed package/environment can possibly contain multiple binaries and
they will be listed out in the command output. Here is an example of a few installed packages:

```
> pixi global list
Global install location: /home/hanabi/.pixi
├── bat 0.24.0
|   └─ exec: bat
├── conda-smithy 3.31.1
|   └─ exec: feedstocks, conda-smithy
├── rattler-build 0.13.0
|   └─ exec: rattler-build
├── ripgrep 14.1.0
|   └─ exec: rg
└── uv 0.1.17
    └─ exec: uv
```

### `global upgrade`

This command upgrades a globally installed package (to the latest version by default).

##### Arguments

1. `<PACKAGE>`: The package to upgrade.

##### Options

- `--channel <CHANNEL> (-c)`: specify a channel that the project uses.
  Defaults to `conda-forge`. Note the channel the package was installed from
  will be always used for upgrade. (Allowed to be used more than once)

```shell
pixi global upgrade ruff
pixi global upgrade --channel conda-forge --channel bioconda trackplot
# Or in a more concise form
pixi global upgrade -c conda-forge -c bioconda trackplot

# Conda matchspec is supported
# You can specify the version to upgrade to when you don't want the latest version
# or you can even use it to downgrade a globally installed package
pixi global upgrade python=3.10
```

### `global upgrade-all`

This command upgrades all globally installed packages to their latest version.

##### Options

- `--channel <CHANNEL> (-c)`: specify a channel that the project uses.
  Defaults to `conda-forge`. Note the channel the package was installed from
  will be always used for upgrade. (Allowed to be used more than once)

```shell
pixi global upgrade-all
pixi global upgrade-all --channel conda-forge --channel bioconda
# Or in a more concise form
pixi global upgrade-all -c conda-forge -c bioconda trackplot
```

### `global remove`

Removes a package previously installed into a globally accessible location via
`pixi global install`

Use `pixi global info` to find out what the package name is that belongs to the tool you want to remove.

##### Arguments

1. `<PACKAGE>`: The package(s) to remove.

```shell
pixi global remove pre-commit

# multiple packages can be removed at once
pixi global remove pre-commit starship
```

## `project`

This subcommand allows you to modify the project configuration through the command line interface.

##### Options

- `--manifest-path <MANIFEST_PATH>`: the path to [manifest file](configuration.md), by default it searches for one in the parent directories.

### `project channel add`

Add channels to the channel list in the project configuration.
When you add channels, the channels are tested for existence, added to the lockfile and the environment is reinstalled.

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
pixi project channel add --feature cuda nividia
```

### `project channel list`

List the channels in the project file

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

List the channels in the project file

##### Arguments

1. `<CHANNEL>...`: The channels to remove, name(s) or URL(s).

##### Options

- `--no-install`: do not update the environment, only add changed packages to the lock-file.
- `--feature <FEATURE> (-f)`: The feature for which the channel is removed.

```sh
pixi project channel remove conda-forge
pixi project channel remove https://conda.anaconda.org/conda-forge/
pixi project channel remove --no-install conda-forge
pixi project channel remove --feature cuda nividia
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

### `project platform add`

Adds a platform(s) to the project file and updates the lockfile.

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

List the platforms in the project file.

```sh
$ pixi project platform list
osx-64
linux-64
win-64
osx-arm64
```

### `project platform remove`

Remove platform(s) from the project file and updates the lockfile.

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
    An **up-to-date** lockfile means that the dependencies in the lockfile are allowed by the dependencies in the manifest file.
    For example

    - a manifest with `python = ">= 3.11"` is up-to-date with a `name: python, version: 3.11.0` in the `pixi.lock`.
    - a manifest with `python = ">= 3.12"` is **not** up-to-date with a `name: python, version: 3.11.0` in the `pixi.lock`.

    Being up-to-date does **not** mean that the lockfile holds the latest version available on the channel for the given dependency.
