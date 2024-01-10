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

## `init`

This command is used to create a new project.
It initializes a `pixi.toml` file and also prepares a `.gitignore` to prevent the environment from being added to `git`.

##### Options

- `--channel (-c)`: specify a channel that the project uses. Defaults to `conda-forge`. (Allowed to be used more than once)

```shell
pixi init myproject
pixi init ~/myproject
pixi init  # Initializes directly in the current directory.
pixi init --channel conda-forge --channel bioconda myproject
```

## `add`

Adds dependencies to the `pixi.toml`.
It will only add if the package with its version constraint is able to work with rest of the dependencies in the project.
[More info](advanced/multi_platform_configuration.md) on multi-platform configuration.

##### Options

- `--manifest-path`: the path to `pixi.toml`, by default it searches for one in the parent directories.
- `--host`: Specifies a host dependency, important for building a package.
- `--build`: Specifies a build dependency, important for building a package.
- `--pypi`: Specifies a PyPI dependency, not a conda package.
      Parses dependencies as [PEP508](https://peps.python.org/pep-0508/) requirements, supporting extras and versions.
      See [configuration](configuration.md) for details.
- `--no-install`: Don't install the package to the environment, only add the package to the lock-file.
- `--platform (-p)`: The platform for which the dependency should be added. (Allowed to be used more than once)

```shell
pixi add numpy
pixi add numpy pandas "pytorch>=1.8"
pixi add "numpy>=1.22,<1.24"
pixi add --manifest-path ~/myproject/pixi.toml numpy
pixi add --host "python>=3.9.0"
pixi add --build cmake
pixi add --pypi requests[security]
pixi add --platform osx-64 --build clang
```

## `install`

Installs all dependencies specified in the lockfile `pixi.lock`.
Which gets generated on `pixi add` or when you manually change the `pixi.toml` file and run `pixi install`.

##### Options

- `--manifest-path`: the path to `pixi.toml`, by default it searches for one in the parent directories.
- `--frozen`: install the environment as defined in the lockfile. Without checking the status of the lockfile.
- `--locked`: only install if the `pixi.lock` is up-to-date with the `pixi.toml`[^1]. Conflicts with `--frozen`.

```shell
pixi install
pixi install --manifest-path ~/myproject/pixi.toml
pixi install --frozen
pixi install --locked
```

## `run`

The `run` commands first checks if the environment is ready to use.
When you didn't run `pixi install` the run command will do that for you.
The custom tasks defined in the `pixi.toml` are also available through the run command.

You cannot run `pixi run source setup.bash` as `source` is not available in the `deno_task_shell` commandos and not an executable.

##### Options

- `--manifest-path`: the path to `pixi.toml`, by default it searches for one in the parent directories.
- `--frozen`: install the environment as defined in the lockfile. Without checking the status of the lockfile.
- `--locked`: only install if the `pixi.lock` is up-to-date with the `pixi.toml`[^1]. Conflicts with `--frozen`.

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
```

!!! info
      In `pixi` the [`deno_task_shell`](https://deno.land/manual@v1.35.0/tools/task_runner#task-runner) is the underlying runner of the run command.
      Checkout their [documentation](https://deno.land/manual@v1.35.0/tools/task_runner#task-runner) for the syntax and available commands.
      This is done so that the run commands can be run across all platforms.


## `task`

If you want to make a shorthand for a specific command you can add a task for it.

##### Options

- `--manifest-path`: the path to `pixi.toml`, by default it searches for one in the parent directories.

### `task add`

Add a task to the `pixi.toml`, use `--depends-on` to add tasks you want to run before this task, e.g. build before an execute task.

##### Options
- `--platform`: the platform for which this task should be added.
- `--depends-on`: the task it depends on to be run before the one your adding.
- `--cwd`: the working directory for the task relative to the root of the project.

```shell
pixi task add cow cowpy "Hello User"
pixi task add tls ls --cwd tests
pixi task add test cargo t --depends-on build
pixi task add build-osx "METAL=1 cargo build" --platform osx-64
```

This adds the following to the `pixi.toml`:

```toml
[tasks]
cow = "cowpy \"Hello User\""
tls = { cmd = "ls", cwd = "tests" }
test = { cmd = "cargo t", depends_on = ["build"] }

[target.osx-64.tasks]
build-osx = "METAL=1 cargo build"
```

Which you can then run with the `run` command:

```shell
pixi run cow
# Extra arguments will be passed to the tasks command.
pixi run test --test test1
```

### `task remove`

Remove the task from the `pixi.toml`

```shell
pixi task remove cow
```

### `task alias`

Give a task a new name or concatenate multiple tasks into one name.

```shell
pixi task alias moo cow
```

Adds the last line to the `pixi.toml`:

```toml
[tasks]
cow = "cowpy \"Hello User\""
moo = { depends_on = ["cow"] }
```

!!! info
      In `pixi` the [`deno_task_shell`](https://deno.land/manual@v1.35.0/tools/task_runner#task-runner) is the underlying runner of the tasks.
      Checkout their [documentation](https://deno.land/manual@v1.35.0/tools/task_runner#task-runner) for the syntax and available commands.
      This is done so that the tasks defined can be run across all platforms.


## `shell`

This command starts a new shell in the project's environment.
To exit the pixi shell, simply run `exit`.

#####Options

- `--manifest-path`: the path to `pixi.toml`, by default it searches for one in the parent directories.
- `--frozen`: install the environment as defined in the lockfile. Without checking the status of the lockfile.
- `--locked`: only install if the `pixi.lock` is up-to-date with the `pixi.toml`[^1]. Conflicts with `--frozen`.

```shell
pixi shell
exit
pixi shell --manifest-path ~/myproject/pixi.toml
exit
pixi shell --frozen
exit
pixi shell --locked
exit
```
## `search`
Search a package, output will list the latest version of the package.

###### Options
- `--manifest-path`: the path to `pixi.toml`, by default it searches for one in the parent directories.
- `--channel (-c)`: specify a channel that the project uses. Defaults to `conda-forge`. (Allowed to be used more than once)
- `--limit (-l)`: Limit the number of search results (default: 15)

```zsh
pixi search pixi
pixi search -l 30 py
pixi search -c robostack plotjuggler
```

## `info`

Shows helpful information about the pixi installation, cache directories, disk usage, and more.
More information [here](advanced/explain_info_command.md).

#####Options

- `--extended`: extend the information with more slow queries to the system, like directory sizes.
- `--json`: Get a machine-readable version of the information as output.

```shell
pixi info
pixi info --json --extended
```

## `upload`

Upload a package to a prefix.dev channel

```shell
pixi upload <HOST> <PACKAGE_FILE>
pixi upload repo.prefix.dev/my_channel my_package.conda
```

## `auth`

This command is used to authenticate the user's access to remote hosts such as `prefix.dev` or `anaconda.org` for private channels.

### `auth login`

Store authentication information for given host.

!!! tip
      The host is real hostname not a channel.


##### Options

- `--token`: The token to use for authentication with prefix.dev.
- `--username`: The username to use for basic HTTP authentication
- `--password`: The password to use for basic HTTP authentication.
- `--conda-token`: The token to use on `anaconda.org` / `quetz` authentication.

```shell
pixi auth login <HOST> [OPTIONS]

pixi auth login repo.prefix.dev --token pfx_JQEV-m_2bdz-D8NSyRSaNdHANx0qHjq7f2iD
pixi auth login anaconda.org --conda-token ABCDEFGHIJKLMNOP
pixi auth login https://myquetz.server --user john --password xxxxxx
```

### `auth logout`

Remove authentication information for a given host.

```shell
pixi auth logout <HOST>
pixi auth logout repo.prefix.dev
pixi auth logout anaconda.org
```

## `global`

Global is the main entry point for the part of pixi that executes on the
global(system) level.

### `global install`

This command installs a package into its own environment and adds the binary to `PATH`, allowing you to access it anywhere on your system without activating the environment.

##### Options

- `--channel (-c)`: specify a channel that the project uses. Defaults to `conda-forge`. (Allowed to be used more than once)

```shell
pixi global install ruff
pixi global install starship
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
A global installed package/environment can possibly contain multiple binaries.
Here is an example of a few installed packages:
```
> pixi global list
Globally installed binary packages:
  -  [package] starship
     -  [bin] starship
  -  [package] pre-commit
     -  [bin] pre-commit
  -  [package] grayskull
     -  [bin] grayskull
     -  [bin] greyskull
     -  [bin] conda-grayskull
     -  [bin] conda-greyskull
  -  [package] zsh
     -  [bin] zsh
     -  [bin] zsh-5
```

### `global remove`

Removes a package previously installed into a globally accessible location via
`pixi global install`

Use `pixi global info` to find out what the package name is that belongs to the tool you want to remove.

```
pixi global remove pre-commit
```

## `project`

This subcommand allows you to modify the project configuration through the command line interface.

##### Options

- `--manifest-path`: the path to `pixi.toml`, by default it searches for one in the parent directories.
- `--no-install`: do not update the environment, only add changed packages to the lock-file.

### `project channel add`

Add channels to the channel list in the project configuration.
When you add channels, the channels are tested for existence, added to the lockfile and the environment is reinstalled.

##### Options

- `--no-install`: do not update the environment, only add changed packages to the lock-file.

```
pixi project channel add robostack
pixi project channel add bioconda conda-forge robostack
pixi project channel add file:///home/user/local_channel
pixi project channel add https://repo.prefix.dev/conda-forge
pixi project channel add --no-install robostack
```

[^1]: An __up-to-date__ lockfile means that the dependencies in the lockfile are allowed by the dependencies in the manifest file.
      For example

      - a `pixi.toml` with `python = ">= 3.11"` is up-to-date with a `name: python, version: 3.11.0` in the `pixi.lock`.
      - a `pixi.toml` with `python = ">= 3.12"` is **not** up-to-date with a `name: python, version: 3.11.0` in the `pixi.lock`.

      Being up-to-date does **not** mean that the lockfile holds the latest version available on the channel for the given dependency.
