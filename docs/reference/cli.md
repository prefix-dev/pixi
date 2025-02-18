# Command-Line Help for `pixi`

This document contains the help content for the `pixi` command-line program.

## `pixi`


Pixi [version 0.41.3] - Developer Workflow and Environment Management for Multi-Platform, Language-Agnostic Projects.

Pixi is a versatile developer workflow tool designed to streamline the management of your project's dependencies, tasks, and environments.
Built on top of the Conda ecosystem, Pixi offers seamless integration with the PyPI ecosystem.

Basic Usage:

    Initialize pixi for a project:
    $ pixi init
    $ pixi add python numpy pytest

    Run a task:
    $ pixi task add test 'pytest -s'
    $ pixi run test

Found a Bug or Have a Feature Request?
Open an issue at: https://github.com/prefix-dev/pixi/issues

Need Help?
Ask a question on the Prefix Discord server: https://discord.gg/kKV8ZxyzY4

For more information, see the documentation at: https://pixi.sh


**Usage:** `pixi [OPTIONS] <COMMAND>`

###### **Subcommands:**

* `init` — Creates a new workspace
* `add` — Adds dependencies to the project
* `remove` — Removes dependencies from the project
* `install` — Install all dependencies
* `update` — Update dependencies as recorded in the local lock file
* `upgrade` — Update the version of packages to the latest possible version, disregarding the manifest version constraints
* `lock` — Solve environment and update the lock file
* `run` — Runs task in project
* `exec` — Run a command in a temporary environment
* `shell` — Start a shell in the pixi environment of the project
* `shell-hook` — Print the pixi environment activation script
* `project` — Modify the project configuration file through the command line
* `task` — Interact with tasks in the project
* `list` — List project's packages
* `tree` — Show a tree of project dependencies
* `global` — Subcommand for global package management actions
* `auth` — Login to prefix.dev or anaconda.org servers to access private channels
* `config` — Configuration management
* `info` — Information about the system, project and environments for the current machine
* `upload` — Upload a conda package
* `search` — Search a conda package
* `clean` — Clean the parts of your system which are touched by pixi. Defaults to cleaning the environments and task cache. Use the `cache` subcommand to clean the cache
* `completion` — Generates a completion script for a shell
* `build` — Workspace configuration

###### **Options:**

* `-v`, `--verbose` — Increase logging verbosity
* `-q`, `--quiet` — Decrease logging verbosity
* `--color <COLOR>` — Whether the log needs to be colored

  Default value: `auto`

  Possible values: `always`, `never`, `auto`

* `--no-progress` — Hide all progress bars, always turned on if stderr is not a terminal

  Default value: `false`



## `pixi init`

Creates a new workspace

It initializes a `pixi.toml` file and also prepares a `.gitignore`
to prevent the environment from being added to `git`.

If you have a `pyproject.toml` file in the directory where you run `pixi init`,
it appends the pixi data to the `pyproject.toml` file instead of creating a new `pixi.toml` file.

When importing an environment, the `pixi.toml` will be created with the dependencies from the environment file.
The `pixi.lock` will be created when you install the environment.
We don't support `git+` urls as dependencies for pip packages and for the `defaults` channel we use `main`,
`r` and `msys2` as the default channels.

    pixi init myproject
    pixi init ~/myproject
    pixi init  # Initializes directly in the current directory.
    pixi init --channel conda-forge --channel bioconda myproject
    pixi init --platform osx-64 --platform linux-64 myproject
    pixi init --import environment.yml
    pixi init --format pyproject
    pixi init --format pixi --scm gitlab

**Usage:** `pixi init [OPTIONS] [PATH]`

###### **Arguments:**

* `<PATH>` — Where to place the project

  Default value: `.`

###### **Options:**

* `-c`, `--channel <channel>` — Channels to use in the project. Defaults to `conda-forge`. (Allowed to be used more than once)
* `-p`, `--platform <platform>` — Platforms that the project supports
* `-i`, `--import <ENV_FILE>` — Environment.yml file to bootstrap the project
* `--format <FORMAT>` — The manifest format to create

  Possible values: `pixi`, `pyproject`

* `-s`, `--scm <SCM>` — Source Control Management used for this project

  Possible values: `github`, `gitlab`, `codeberg`




## `pixi add`

Adds dependencies to the project

The dependencies should be defined as MatchSpec for conda package, or a PyPI
requirement for the `--pypi` dependencies. If no specific version is
provided, the latest version compatible with your project will be chosen
automatically or a * will be used.

Example usage:

- `pixi add python=3.9`: This will select the latest minor version that
  complies with 3.9.*, i.e., python version 3.9.0, 3.9.1, 3.9.2, etc.
- `pixi add python`: In absence of a specified version, the latest version
  will be chosen. For instance, this could resolve to python version
  3.11.3.* at the time of writing.

Adding multiple dependencies at once is also supported:
- `pixi add python pytest`: This will add both `python` and `pytest` to the
  project's dependencies.

The `--platform` and `--build/--host` flags make the dependency target
specific.
- `pixi add python --platform linux-64 --platform osx-arm64`: Will add the
  latest version of python for linux-64 and osx-arm64 platforms.
- `pixi add python --build`: Will add the latest version of python for as a
  build dependency.

Mixing `--platform` and `--build`/`--host` flags is supported

The `--pypi` option will add the package as a pypi dependency. This cannot
be mixed with the conda dependencies
- `pixi add --pypi boto3`
- `pixi add --pypi "boto3==version"

If the project manifest is a `pyproject.toml`, adding a pypi dependency will
add it to the native pyproject `project.dependencies` array or to the native
`dependency-groups` table if a feature is specified:
- `pixi add --pypi boto3` will add `boto3` to the `project.dependencies`
  array
- `pixi add --pypi boto3 --feature aws` will add `boto3` to the
  `dependency-groups.aws` array

Note that if `--platform` or `--editable` are specified, the pypi dependency
will be added to the `tool.pixi.pypi-dependencies` table instead as native
arrays have no support for platform-specific or editable dependencies.

These dependencies will then be read by pixi as if they had been added to
the pixi `pypi-dependencies` tables of the default or of a named feature.

The versions will be automatically added with a pinning strategy based on
semver or the pinning strategy set in the config. There is a list of
packages that are not following the semver versioning scheme but will use
the minor version by default:
Python, Rust, Julia, GCC, GXX, GFortran, NodeJS, Deno, R, R-Base, Perl

**Usage:** `pixi add [OPTIONS] <SPECS>...`

###### **Arguments:**

* `<SPECS>` — The dependencies as names, conda MatchSpecs or PyPi requirements

###### **Options:**

* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory
* `--host` — The specified dependencies are host dependencies. Conflicts with `build` and `pypi`
* `--build` — The specified dependencies are build dependencies. Conflicts with `host` and `pypi`
* `--pypi` — The specified dependencies are pypi dependencies. Conflicts with `host` and `build`
* `-p`, `--platform <PLATFORMS>` — The platform(s) for which the dependency should be modified
* `-f`, `--feature <FEATURE>` — The feature for which the dependency should be modified

  Default value: `default`
* `-g`, `--git <GIT>` — The git url to use when adding a git dependency
* `--branch <BRANCH>` — The git branch
* `--tag <TAG>` — The git tag
* `--rev <REV>` — The git revision
* `-s`, `--subdir <SUBDIR>` — The subdirectory of the git repository to use
* `--no-lockfile-update` — Don't update lockfile, implies the no-install as well
* `--frozen` — Install the environment as defined in the lockfile, doesn't update lockfile if it isn't up-to-date with the manifest file
* `--locked` — Check if lockfile is up-to-date before installing the environment, aborts when lockfile isn't up-to-date with the manifest file
* `--no-install` — Don't modify the environment, only modify the lock-file
* `--tls-no-verify` — Do not verify the TLS certificate of the server
* `--auth-file <AUTH_FILE>` — Path to the file containing the authentication token
* `--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>` — Specifies if we want to use uv keyring provider

  Possible values: `disabled`, `subprocess`

* `--concurrent-solves <CONCURRENT_SOLVES>` — Max concurrent solves, default is the number of CPUs
* `--concurrent-downloads <CONCURRENT_DOWNLOADS>` — Max concurrent network requests, default is 50
* `--revalidate` — Run the complete environment validation. This will reinstall a broken environment
* `--editable` — Whether the pypi requirement should be editable



## `pixi remove`

Removes dependencies from the project

If the project manifest is a `pyproject.toml`, removing a pypi dependency with the `--pypi` flag will remove it from either - the native pyproject `project.dependencies` array or, if a feature is specified, the native `project.optional-dependencies` table - pixi `pypi-dependencies` tables of the default feature or, if a feature is specified, a named feature

**Usage:** `pixi remove [OPTIONS] <SPECS>...`

###### **Arguments:**

* `<SPECS>` — The dependencies as names, conda MatchSpecs or PyPi requirements

###### **Options:**

* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory
* `--host` — The specified dependencies are host dependencies. Conflicts with `build` and `pypi`
* `--build` — The specified dependencies are build dependencies. Conflicts with `host` and `pypi`
* `--pypi` — The specified dependencies are pypi dependencies. Conflicts with `host` and `build`
* `-p`, `--platform <PLATFORMS>` — The platform(s) for which the dependency should be modified
* `-f`, `--feature <FEATURE>` — The feature for which the dependency should be modified

  Default value: `default`
* `-g`, `--git <GIT>` — The git url to use when adding a git dependency
* `--branch <BRANCH>` — The git branch
* `--tag <TAG>` — The git tag
* `--rev <REV>` — The git revision
* `-s`, `--subdir <SUBDIR>` — The subdirectory of the git repository to use
* `--no-lockfile-update` — Don't update lockfile, implies the no-install as well
* `--frozen` — Install the environment as defined in the lockfile, doesn't update lockfile if it isn't up-to-date with the manifest file
* `--locked` — Check if lockfile is up-to-date before installing the environment, aborts when lockfile isn't up-to-date with the manifest file
* `--no-install` — Don't modify the environment, only modify the lock-file
* `--tls-no-verify` — Do not verify the TLS certificate of the server
* `--auth-file <AUTH_FILE>` — Path to the file containing the authentication token
* `--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>` — Specifies if we want to use uv keyring provider

  Possible values: `disabled`, `subprocess`

* `--concurrent-solves <CONCURRENT_SOLVES>` — Max concurrent solves, default is the number of CPUs
* `--concurrent-downloads <CONCURRENT_DOWNLOADS>` — Max concurrent network requests, default is 50
* `--revalidate` — Run the complete environment validation. This will reinstall a broken environment



## `pixi install`

Install all dependencies

**Usage:** `pixi install [OPTIONS]`

###### **Options:**

* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory
* `--frozen` — Install the environment as defined in the lockfile, doesn't update lockfile if it isn't up-to-date with the manifest file
* `--locked` — Check if lockfile is up-to-date before installing the environment, aborts when lockfile isn't up-to-date with the manifest file
* `-e`, `--environment <ENVIRONMENT>` — The environment to install
* `--tls-no-verify` — Do not verify the TLS certificate of the server
* `--auth-file <AUTH_FILE>` — Path to the file containing the authentication token
* `--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>` — Specifies if we want to use uv keyring provider

  Possible values: `disabled`, `subprocess`

* `--concurrent-solves <CONCURRENT_SOLVES>` — Max concurrent solves, default is the number of CPUs
* `--concurrent-downloads <CONCURRENT_DOWNLOADS>` — Max concurrent network requests, default is 50
* `-a`, `--all`



## `pixi update`

Update dependencies as recorded in the local lock file

**Usage:** `pixi update [OPTIONS] [PACKAGES]...`

###### **Arguments:**

* `<PACKAGES>` — The packages to update

###### **Options:**

* `--tls-no-verify` — Do not verify the TLS certificate of the server
* `--auth-file <AUTH_FILE>` — Path to the file containing the authentication token
* `--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>` — Specifies if we want to use uv keyring provider

  Possible values: `disabled`, `subprocess`

* `--concurrent-solves <CONCURRENT_SOLVES>` — Max concurrent solves, default is the number of CPUs
* `--concurrent-downloads <CONCURRENT_DOWNLOADS>` — Max concurrent network requests, default is 50
* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory
* `--no-install` — Don't install the (solve) environments needed for pypi-dependencies solving
* `-n`, `--dry-run` — Don't actually write the lockfile or update any environment
* `-e`, `--environment <ENVIRONMENTS>` — The environments to update. If none is specified, all environments are updated
* `-p`, `--platform <PLATFORMS>` — The platforms to update. If none is specified, all platforms are updated
* `--json` — Output the changes in JSON format



## `pixi upgrade`

Update the version of packages to the latest possible version, disregarding the manifest version constraints

**Usage:** `pixi upgrade [OPTIONS] [PACKAGES]...`

###### **Arguments:**

* `<PACKAGES>` — The packages to upgrade

###### **Options:**

* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory
* `--no-lockfile-update` — Don't update lockfile, implies the no-install as well
* `--frozen` — Install the environment as defined in the lockfile, doesn't update lockfile if it isn't up-to-date with the manifest file
* `--locked` — Check if lockfile is up-to-date before installing the environment, aborts when lockfile isn't up-to-date with the manifest file
* `--no-install` — Don't modify the environment, only modify the lock-file
* `--tls-no-verify` — Do not verify the TLS certificate of the server
* `--auth-file <AUTH_FILE>` — Path to the file containing the authentication token
* `--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>` — Specifies if we want to use uv keyring provider

  Possible values: `disabled`, `subprocess`

* `--concurrent-solves <CONCURRENT_SOLVES>` — Max concurrent solves, default is the number of CPUs
* `--concurrent-downloads <CONCURRENT_DOWNLOADS>` — Max concurrent network requests, default is 50
* `--revalidate` — Run the complete environment validation. This will reinstall a broken environment
* `-f`, `--feature <FEATURE>` — The feature to update

  Default value: `default`
* `--exclude <EXCLUDE>` — The packages which should be excluded
* `--json` — Output the changes in JSON format
* `-n`, `--dry-run` — Only show the changes that would be made, without actually updating the manifest, lock file, or environment



## `pixi lock`

Solve environment and update the lock file

**Usage:** `pixi lock [OPTIONS]`

###### **Options:**

* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory
* `--json` — Output the changes in JSON format



## `pixi run`

Runs task in project

**Usage:** `pixi run [OPTIONS] [TASK]...`

###### **Arguments:**

* `<TASK>` — The pixi task or a task shell command you want to run in the project's environment, which can be an executable in the environment's PATH

###### **Options:**

* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory
* `--no-lockfile-update` — Don't update lockfile, implies the no-install as well
* `--frozen` — Install the environment as defined in the lockfile, doesn't update lockfile if it isn't up-to-date with the manifest file
* `--locked` — Check if lockfile is up-to-date before installing the environment, aborts when lockfile isn't up-to-date with the manifest file
* `--no-install` — Don't modify the environment, only modify the lock-file
* `--tls-no-verify` — Do not verify the TLS certificate of the server
* `--auth-file <AUTH_FILE>` — Path to the file containing the authentication token
* `--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>` — Specifies if we want to use uv keyring provider

  Possible values: `disabled`, `subprocess`

* `--concurrent-solves <CONCURRENT_SOLVES>` — Max concurrent solves, default is the number of CPUs
* `--concurrent-downloads <CONCURRENT_DOWNLOADS>` — Max concurrent network requests, default is 50
* `--revalidate` — Run the complete environment validation. This will reinstall a broken environment
* `--force-activate` — Do not use the environment activation cache. (default: true except in experimental mode)
* `-e`, `--environment <ENVIRONMENT>` — The environment to run the task in
* `--clean-env` — Use a clean environment to run the task

   Using this flag will ignore your current shell environment and use bare minimum environment to activate the pixi environment in.
* `--skip-deps` — Don't run the dependencies of the task ('depends-on' field in the task definition)
* `-n`, `--dry-run` — Run the task in dry-run mode (only print the command that would run)
* `--help`

  Possible values: `true`, `false`

* `-h`

  Possible values: `true`, `false`




## `pixi exec`

Run a command in a temporary environment

**Usage:** `pixi exec [OPTIONS] [COMMAND]...`

###### **Arguments:**

* `<COMMAND>` — The executable to run

###### **Options:**

* `-s`, `--spec <SPECS>` — Matchspecs of packages to install. If this is not provided, the package is guessed from the command
* `-c`, `--channel <CHANNEL>` — The channels to consider as a name or a url. Multiple channels can be specified by using this field multiple times.

   When specifying a channel, it is common that the selected channel also depends on the `conda-forge` channel.

   By default, if no channel is provided, `conda-forge` is used.
* `-p`, `--platform <PLATFORM>` — The platform to create the environment for

  Default value: `linux-64`
* `--force-reinstall` — If specified a new environment is always created even if one already exists
* `--tls-no-verify` — Do not verify the TLS certificate of the server
* `--auth-file <AUTH_FILE>` — Path to the file containing the authentication token
* `--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>` — Specifies if we want to use uv keyring provider

  Possible values: `disabled`, `subprocess`

* `--concurrent-solves <CONCURRENT_SOLVES>` — Max concurrent solves, default is the number of CPUs
* `--concurrent-downloads <CONCURRENT_DOWNLOADS>` — Max concurrent network requests, default is 50



## `pixi shell`

Start a shell in the pixi environment of the project

**Usage:** `pixi shell [OPTIONS]`

###### **Options:**

* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory
* `--no-lockfile-update` — Don't update lockfile, implies the no-install as well
* `--frozen` — Install the environment as defined in the lockfile, doesn't update lockfile if it isn't up-to-date with the manifest file
* `--locked` — Check if lockfile is up-to-date before installing the environment, aborts when lockfile isn't up-to-date with the manifest file
* `--no-install` — Don't modify the environment, only modify the lock-file
* `--tls-no-verify` — Do not verify the TLS certificate of the server
* `--auth-file <AUTH_FILE>` — Path to the file containing the authentication token
* `--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>` — Specifies if we want to use uv keyring provider

  Possible values: `disabled`, `subprocess`

* `--concurrent-solves <CONCURRENT_SOLVES>` — Max concurrent solves, default is the number of CPUs
* `--concurrent-downloads <CONCURRENT_DOWNLOADS>` — Max concurrent network requests, default is 50
* `--revalidate` — Run the complete environment validation. This will reinstall a broken environment
* `-e`, `--environment <ENVIRONMENT>` — The environment to activate in the shell
* `--change-ps1 <CHANGE_PS1>` — Do not change the PS1 variable when starting a prompt

  Possible values: `true`, `false`

* `--force-activate` — Do not use the environment activation cache. (default: true except in experimental mode)



## `pixi shell-hook`

Print the pixi environment activation script.

You can source the script to activate the environment without needing pixi itself.

**Usage:** `pixi shell-hook [OPTIONS]`

###### **Options:**

* `-s`, `--shell <SHELL>` — Sets the shell, options: [`bash`,  `zsh`,  `xonsh`,  `cmd`, `powershell`,  `fish`,  `nushell`]
* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory
* `--no-lockfile-update` — Don't update lockfile, implies the no-install as well
* `--frozen` — Install the environment as defined in the lockfile, doesn't update lockfile if it isn't up-to-date with the manifest file
* `--locked` — Check if lockfile is up-to-date before installing the environment, aborts when lockfile isn't up-to-date with the manifest file
* `--no-install` — Don't modify the environment, only modify the lock-file
* `--tls-no-verify` — Do not verify the TLS certificate of the server
* `--auth-file <AUTH_FILE>` — Path to the file containing the authentication token
* `--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>` — Specifies if we want to use uv keyring provider

  Possible values: `disabled`, `subprocess`

* `--concurrent-solves <CONCURRENT_SOLVES>` — Max concurrent solves, default is the number of CPUs
* `--concurrent-downloads <CONCURRENT_DOWNLOADS>` — Max concurrent network requests, default is 50
* `--revalidate` — Run the complete environment validation. This will reinstall a broken environment
* `--force-activate` — Do not use the environment activation cache. (default: true except in experimental mode)
* `-e`, `--environment <ENVIRONMENT>` — The environment to activate in the script
* `--json` — Emit the environment variables set by running the activation as JSON

  Default value: `false`
* `--change-ps1 <CHANGE_PS1>` — Do not change the PS1 variable when starting a prompt

  Possible values: `true`, `false`




## `pixi project`

Modify the project configuration file through the command line

**Usage:** `pixi project [OPTIONS] <COMMAND>`

###### **Subcommands:**

* `channel` — Commands to manage project channels
* `description` — Commands to manage project description
* `platform` — Commands to manage project platforms
* `version` — Commands to manage project version
* `environment` — Commands to manage project environments
* `export` — Commands to export projects to other formats
* `name` — Commands to manage project name
* `system-requirements` — Commands to manage project environments

###### **Options:**

* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory



## `pixi project channel`

Commands to manage project channels

**Usage:** `pixi project channel <COMMAND>`

###### **Subcommands:**

* `add` — Adds a channel to the project file and updates the lockfile
* `list` — List the channels in the project file
* `remove` — Remove channel(s) from the project file and updates the lockfile



## `pixi project channel add`

Adds a channel to the project file and updates the lockfile

**Usage:** `pixi project channel add [OPTIONS] <CHANNEL>...`

###### **Arguments:**

* `<CHANNEL>` — The channel name or URL

###### **Options:**

* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory
* `--priority <PRIORITY>` — Specify the channel priority
* `--prepend` — Add the channel(s) to the beginning of the channels list, making them the highest priority
* `--no-lockfile-update` — Don't update lockfile, implies the no-install as well
* `--frozen` — Install the environment as defined in the lockfile, doesn't update lockfile if it isn't up-to-date with the manifest file
* `--locked` — Check if lockfile is up-to-date before installing the environment, aborts when lockfile isn't up-to-date with the manifest file
* `--no-install` — Don't modify the environment, only modify the lock-file
* `--tls-no-verify` — Do not verify the TLS certificate of the server
* `--auth-file <AUTH_FILE>` — Path to the file containing the authentication token
* `--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>` — Specifies if we want to use uv keyring provider

  Possible values: `disabled`, `subprocess`

* `--concurrent-solves <CONCURRENT_SOLVES>` — Max concurrent solves, default is the number of CPUs
* `--concurrent-downloads <CONCURRENT_DOWNLOADS>` — Max concurrent network requests, default is 50
* `--revalidate` — Run the complete environment validation. This will reinstall a broken environment
* `-f`, `--feature <FEATURE>` — The name of the feature to modify



## `pixi project channel list`

List the channels in the project file

**Usage:** `pixi project channel list [OPTIONS]`

###### **Options:**

* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory
* `--urls` — Whether to display the channel's names or urls



## `pixi project channel remove`

Remove channel(s) from the project file and updates the lockfile

**Usage:** `pixi project channel remove [OPTIONS] <CHANNEL>...`

###### **Arguments:**

* `<CHANNEL>` — The channel name or URL

###### **Options:**

* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory
* `--priority <PRIORITY>` — Specify the channel priority
* `--prepend` — Add the channel(s) to the beginning of the channels list, making them the highest priority
* `--no-lockfile-update` — Don't update lockfile, implies the no-install as well
* `--frozen` — Install the environment as defined in the lockfile, doesn't update lockfile if it isn't up-to-date with the manifest file
* `--locked` — Check if lockfile is up-to-date before installing the environment, aborts when lockfile isn't up-to-date with the manifest file
* `--no-install` — Don't modify the environment, only modify the lock-file
* `--tls-no-verify` — Do not verify the TLS certificate of the server
* `--auth-file <AUTH_FILE>` — Path to the file containing the authentication token
* `--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>` — Specifies if we want to use uv keyring provider

  Possible values: `disabled`, `subprocess`

* `--concurrent-solves <CONCURRENT_SOLVES>` — Max concurrent solves, default is the number of CPUs
* `--concurrent-downloads <CONCURRENT_DOWNLOADS>` — Max concurrent network requests, default is 50
* `--revalidate` — Run the complete environment validation. This will reinstall a broken environment
* `-f`, `--feature <FEATURE>` — The name of the feature to modify



## `pixi project description`

Commands to manage project description

**Usage:** `pixi project description [OPTIONS] <COMMAND>`

###### **Subcommands:**

* `get` — Get the project description
* `set` — Set the project description

###### **Options:**

* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory



## `pixi project description get`

Get the project description

**Usage:** `pixi project description get`



## `pixi project description set`

Set the project description

**Usage:** `pixi project description set <DESCRIPTION>`

###### **Arguments:**

* `<DESCRIPTION>` — The project description



## `pixi project platform`

Commands to manage project platforms

**Usage:** `pixi project platform [OPTIONS] <COMMAND>`

###### **Subcommands:**

* `add` — Adds a platform(s) to the project file and updates the lockfile
* `list` — List the platforms in the project file
* `remove` — Remove platform(s) from the project file and updates the lockfile

###### **Options:**

* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory



## `pixi project platform add`

Adds a platform(s) to the project file and updates the lockfile

**Usage:** `pixi project platform add [OPTIONS] <PLATFORM>...`

###### **Arguments:**

* `<PLATFORM>` — The platform name(s) to add

###### **Options:**

* `--no-install` — Don't update the environment, only add changed packages to the lock-file
* `-f`, `--feature <FEATURE>` — The name of the feature to add the platform to



## `pixi project platform list`

List the platforms in the project file

**Usage:** `pixi project platform list`



## `pixi project platform remove`

Remove platform(s) from the project file and updates the lockfile

**Usage:** `pixi project platform remove [OPTIONS] <PLATFORMS>...`

###### **Arguments:**

* `<PLATFORMS>` — The platform name(s) to remove

###### **Options:**

* `--no-install` — Don't update the environment, only remove the platform(s) from the lock-file
* `-f`, `--feature <FEATURE>` — The name of the feature to remove the platform from



## `pixi project version`

Commands to manage project version

**Usage:** `pixi project version [OPTIONS] <COMMAND>`

###### **Subcommands:**

* `get` — Get the workspace version
* `set` — Set the workspace version
* `major` — Bump the workspace version to MAJOR
* `minor` — Bump the workspace version to MINOR
* `patch` — Bump the workspace version to PATCH

###### **Options:**

* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory



## `pixi project version get`

Get the workspace version

**Usage:** `pixi project version get`



## `pixi project version set`

Set the workspace version

**Usage:** `pixi project version set <VERSION>`

###### **Arguments:**

* `<VERSION>` — The new project version



## `pixi project version major`

Bump the workspace version to MAJOR

**Usage:** `pixi project version major`



## `pixi project version minor`

Bump the workspace version to MINOR

**Usage:** `pixi project version minor`



## `pixi project version patch`

Bump the workspace version to PATCH

**Usage:** `pixi project version patch`



## `pixi project environment`

Commands to manage project environments

**Usage:** `pixi project environment [OPTIONS] <COMMAND>`

###### **Subcommands:**

* `add` — Adds an environment to the manifest file
* `list` — List the environments in the manifest file
* `remove` — Remove an environment from the manifest file

###### **Options:**

* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory



## `pixi project environment add`

Adds an environment to the manifest file

**Usage:** `pixi project environment add [OPTIONS] <NAME>`

###### **Arguments:**

* `<NAME>` — The name of the environment to add

###### **Options:**

* `-f`, `--feature <FEATURES>` — Features to add to the environment
* `--solve-group <SOLVE_GROUP>` — The solve-group to add the environment to
* `--no-default-feature` — Don't include the default feature in the environment

  Default value: `false`
* `--force` — Update the manifest even if the environment already exists

  Default value: `false`



## `pixi project environment list`

List the environments in the manifest file

**Usage:** `pixi project environment list`



## `pixi project environment remove`

Remove an environment from the manifest file

**Usage:** `pixi project environment remove <NAME>`

###### **Arguments:**

* `<NAME>` — The name of the environment to remove



## `pixi project export`

Commands to export projects to other formats

**Usage:** `pixi project export <COMMAND>`

###### **Subcommands:**

* `conda-explicit-spec` — Export project environment to a conda explicit specification file
* `conda-environment` — Export project environment to a conda environment.yaml file



## `pixi project export conda-explicit-spec`

Export project environment to a conda explicit specification file

**Usage:** `pixi project export conda-explicit-spec [OPTIONS] <OUTPUT_DIR>`

###### **Arguments:**

* `<OUTPUT_DIR>` — Output directory for rendered explicit environment spec files

###### **Options:**

* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory
* `-e`, `--environment <ENVIRONMENT>`
* `-p`, `--platform <PLATFORM>` — The platform to render. Can be repeated for multiple platforms. Defaults to all platforms available for selected environments
* `--ignore-pypi-errors` — PyPI dependencies are not supported in the conda explicit spec file

  Default value: `false`
* `--ignore-source-errors` — Source dependencies are not supported in the conda explicit spec file

  Default value: `false`
* `--no-lockfile-update` — Don't update lockfile, implies the no-install as well
* `--frozen` — Install the environment as defined in the lockfile, doesn't update lockfile if it isn't up-to-date with the manifest file
* `--locked` — Check if lockfile is up-to-date before installing the environment, aborts when lockfile isn't up-to-date with the manifest file
* `--no-install` — Don't modify the environment, only modify the lock-file
* `--tls-no-verify` — Do not verify the TLS certificate of the server
* `--auth-file <AUTH_FILE>` — Path to the file containing the authentication token
* `--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>` — Specifies if we want to use uv keyring provider

  Possible values: `disabled`, `subprocess`

* `--concurrent-solves <CONCURRENT_SOLVES>` — Max concurrent solves, default is the number of CPUs
* `--concurrent-downloads <CONCURRENT_DOWNLOADS>` — Max concurrent network requests, default is 50
* `--revalidate` — Run the complete environment validation. This will reinstall a broken environment



## `pixi project export conda-environment`

Export project environment to a conda environment.yaml file

**Usage:** `pixi project export conda-environment [OPTIONS] [OUTPUT_PATH]`

###### **Arguments:**

* `<OUTPUT_PATH>` — Explicit path to export the environment to

###### **Options:**

* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory
* `-p`, `--platform <PLATFORM>` — The platform to render the environment file for. Defaults to the current platform
* `-e`, `--environment <ENVIRONMENT>` — The environment to render the environment file for. Defaults to the default environment



## `pixi project name`

Commands to manage project name

**Usage:** `pixi project name [OPTIONS] <COMMAND>`

###### **Subcommands:**

* `get` — Get the project name
* `set` — Set the project name

###### **Options:**

* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory



## `pixi project name get`

Get the project name

**Usage:** `pixi project name get`



## `pixi project name set`

Set the project name

**Usage:** `pixi project name set <NAME>`

###### **Arguments:**

* `<NAME>` — The project name



## `pixi project system-requirements`

Commands to manage project environments

**Usage:** `pixi project system-requirements [OPTIONS] <COMMAND>`

###### **Subcommands:**

* `add` — Adds an environment to the manifest file
* `list` — List the environments in the manifest file

###### **Options:**

* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory



## `pixi project system-requirements add`

Adds an environment to the manifest file

**Usage:** `pixi project system-requirements add [OPTIONS] <REQUIREMENT> <VERSION>`

###### **Arguments:**

* `<REQUIREMENT>` — The name of the system requirement to add

  Possible values:
  - `linux`:
    The version of the linux kernel (Find with `uname -r`)
  - `cuda`:
    The version of the CUDA driver (Find with `nvidia-smi`)
  - `macos`:
    The version of MacOS (Find with `sw_vers`)
  - `glibc`:
    The version of the glibc library (Find with `ldd --version`)
  - `other-libc`:
    Non Glibc libc family and version (Find with `ldd --version`)

* `<VERSION>` — The version of the requirement

###### **Options:**

* `--family <FAMILY>` — The Libc family, this can only be specified for requirement `other-libc`
* `-f`, `--feature <FEATURE>` — The name of the feature to modify



## `pixi project system-requirements list`

List the environments in the manifest file

**Usage:** `pixi project system-requirements list [OPTIONS]`

###### **Options:**

* `--json`
* `-e`, `--environment <ENVIRONMENT>`



## `pixi task`

Interact with tasks in the project

**Usage:** `pixi task [OPTIONS] <COMMAND>`

###### **Subcommands:**

* `add` — Add a command to the project
* `remove` — Remove a command from the project
* `alias` — Alias another specific command
* `list` — List all tasks in the project

###### **Options:**

* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory



## `pixi task add`

Add a command to the project

**Usage:** `pixi task add [OPTIONS] <NAME> <COMMANDS>...`

###### **Arguments:**

* `<NAME>` — Task name
* `<COMMANDS>` — One or more commands to actually execute

###### **Options:**

* `--depends-on <DEPENDS_ON>` — Depends on these other commands
* `-p`, `--platform <PLATFORM>` — The platform for which the task should be added
* `-f`, `--feature <FEATURE>` — The feature for which the task should be added
* `--cwd <CWD>` — The working directory relative to the root of the project
* `--env <ENV>` — The environment variable to set, use --env key=value multiple times for more than one variable
* `--description <DESCRIPTION>` — A description of the task to be added
* `--clean-env` — Isolate the task from the shell environment, and only use the pixi environment to run the task



## `pixi task remove`

Remove a command from the project

**Usage:** `pixi task remove [OPTIONS] [NAMES]...`

###### **Arguments:**

* `<NAMES>` — Task names to remove

###### **Options:**

* `-p`, `--platform <PLATFORM>` — The platform for which the task should be removed
* `-f`, `--feature <FEATURE>` — The feature for which the task should be removed



## `pixi task alias`

Alias another specific command

**Usage:** `pixi task alias [OPTIONS] <ALIAS> <DEPENDS_ON>...`

###### **Arguments:**

* `<ALIAS>` — Alias name
* `<DEPENDS_ON>` — Depends on these tasks to execute

###### **Options:**

* `-p`, `--platform <PLATFORM>` — The platform for which the alias should be added
* `--description <DESCRIPTION>` — The description of the alias task



## `pixi task list`

List all tasks in the project

**Usage:** `pixi task list [OPTIONS]`

###### **Options:**

* `-s`, `--summary` — Tasks available for this machine per environment
* `-e`, `--environment <ENVIRONMENT>` — The environment the list should be generated for. If not specified, the default environment is used
* `--json` — List as json instead of a tree If not specified, the default environment is used



## `pixi list`

List project's packages.

Highlighted packages are explicit dependencies.

**Usage:** `pixi list [OPTIONS] [REGEX]`

###### **Arguments:**

* `<REGEX>` — List only packages matching a regular expression

###### **Options:**

* `--platform <PLATFORM>` — The platform to list packages for. Defaults to the current platform
* `--json` — Whether to output in json format
* `--json-pretty` — Whether to output in pretty json format
* `--sort-by <SORT_BY>` — Sorting strategy

  Default value: `name`

  Possible values: `size`, `name`, `kind`

* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory
* `-e`, `--environment <ENVIRONMENT>` — The environment to list packages for. Defaults to the default environment
* `--no-lockfile-update` — Don't update lockfile, implies the no-install as well
* `--frozen` — Install the environment as defined in the lockfile, doesn't update lockfile if it isn't up-to-date with the manifest file
* `--locked` — Check if lockfile is up-to-date before installing the environment, aborts when lockfile isn't up-to-date with the manifest file
* `--no-install` — Don't modify the environment, only modify the lock-file
* `--tls-no-verify` — Do not verify the TLS certificate of the server
* `--auth-file <AUTH_FILE>` — Path to the file containing the authentication token
* `--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>` — Specifies if we want to use uv keyring provider

  Possible values: `disabled`, `subprocess`

* `--concurrent-solves <CONCURRENT_SOLVES>` — Max concurrent solves, default is the number of CPUs
* `--concurrent-downloads <CONCURRENT_DOWNLOADS>` — Max concurrent network requests, default is 50
* `--revalidate` — Run the complete environment validation. This will reinstall a broken environment
* `-x`, `--explicit` — Only list packages that are explicitly defined in the project



## `pixi tree`

Show a tree of project dependencies

Dependency names highlighted in green are directly specified in the manifest. Yellow version numbers are conda packages, PyPI version numbers are blue.


**Usage:** `pixi tree [OPTIONS] [REGEX]`

###### **Arguments:**

* `<REGEX>` — List only packages matching a regular expression

###### **Options:**

* `-p`, `--platform <PLATFORM>` — The platform to list packages for. Defaults to the current platform
* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory
* `-e`, `--environment <ENVIRONMENT>` — The environment to list packages for. Defaults to the default environment
* `--no-lockfile-update` — Don't update lockfile, implies the no-install as well
* `--frozen` — Install the environment as defined in the lockfile, doesn't update lockfile if it isn't up-to-date with the manifest file
* `--locked` — Check if lockfile is up-to-date before installing the environment, aborts when lockfile isn't up-to-date with the manifest file
* `--no-install` — Don't modify the environment, only modify the lock-file
* `--tls-no-verify` — Do not verify the TLS certificate of the server
* `--auth-file <AUTH_FILE>` — Path to the file containing the authentication token
* `--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>` — Specifies if we want to use uv keyring provider

  Possible values: `disabled`, `subprocess`

* `--concurrent-solves <CONCURRENT_SOLVES>` — Max concurrent solves, default is the number of CPUs
* `--concurrent-downloads <CONCURRENT_DOWNLOADS>` — Max concurrent network requests, default is 50
* `--revalidate` — Run the complete environment validation. This will reinstall a broken environment
* `-i`, `--invert` — Invert tree and show what depends on given package in the regex argument



## `pixi global`

Subcommand for global package management actions

Install packages on the user level. Example: pixi global install my_package pixi global remove my_package

**Usage:** `pixi global <COMMAND>`

###### **Subcommands:**

* `add` — Adds dependencies to an environment
* `edit` — Edit the global manifest file
* `install` — Installs the defined packages in a globally accessible location and exposes their command line applications.
* `uninstall` — Uninstalls environments from the global environment.
* `remove` — Removes dependencies from an environment
* `list` — Lists all packages previously installed into a globally accessible location via `pixi global install`.
* `sync` — Sync global manifest with installed environments
* `expose` — Interact with the exposure of binaries in the global environment
* `update` — Updates environments in the global environment



## `pixi global add`

Adds dependencies to an environment

Example:
- pixi global add --environment python numpy
- pixi global add --environment my_env pytest pytest-cov --expose pytest=pytest

**Usage:** `pixi global add [OPTIONS] --environment <ENVIRONMENT> <PACKAGES>...`

###### **Arguments:**

* `<PACKAGES>` — Specifies the packages that are to be added to the environment

###### **Options:**

* `-e`, `--environment <ENVIRONMENT>` — Specifies the environment that the dependencies need to be added to
* `--expose <EXPOSE>` — Add one or more mapping which describe which executables are exposed. The syntax is `exposed_name=executable_name`, so for example `python3.10=python`. Alternatively, you can input only an executable_name and `executable_name=executable_name` is assumed
* `--tls-no-verify` — Do not verify the TLS certificate of the server
* `--auth-file <AUTH_FILE>` — Path to the file containing the authentication token
* `--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>` — Specifies if we want to use uv keyring provider

  Possible values: `disabled`, `subprocess`

* `--concurrent-solves <CONCURRENT_SOLVES>` — Max concurrent solves, default is the number of CPUs
* `--concurrent-downloads <CONCURRENT_DOWNLOADS>` — Max concurrent network requests, default is 50



## `pixi global edit`

Edit the global manifest file

Opens your editor to edit the global manifest file.

**Usage:** `pixi global edit [EDITOR]`

###### **Arguments:**

* `<EDITOR>` — The editor to use, defaults to `EDITOR` environment variable or `nano` on Unix and `notepad` on Windows



## `pixi global install`

Installs the defined packages in a globally accessible location and exposes their command line applications.

Example:
- pixi global install starship nushell ripgrep bat
- pixi global install jupyter --with polars
- pixi global install --expose python3.8=python python=3.8
- pixi global install --environment science --expose jupyter --expose ipython jupyter ipython polars

**Usage:** `pixi global install [OPTIONS] <PACKAGES>...`

###### **Arguments:**

* `<PACKAGES>` — Specifies the packages that are to be installed

###### **Options:**

* `-c`, `--channel <CHANNEL>` — The channels to consider as a name or a url. Multiple channels can be specified by using this field multiple times.

   When specifying a channel, it is common that the selected channel also depends on the `conda-forge` channel.

   By default, if no channel is provided, `conda-forge` is used.
* `-p`, `--platform <PLATFORM>`
* `-e`, `--environment <ENVIRONMENT>` — Ensures that all packages will be installed in the same environment
* `--expose <EXPOSE>` — Add one or more mapping which describe which executables are exposed. The syntax is `exposed_name=executable_name`, so for example `python3.10=python`. Alternatively, you can input only an executable_name and `executable_name=executable_name` is assumed
* `--with <WITH>` — Add additional dependencies to the environment. Their executables will not be exposed
* `--tls-no-verify` — Do not verify the TLS certificate of the server
* `--auth-file <AUTH_FILE>` — Path to the file containing the authentication token
* `--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>` — Specifies if we want to use uv keyring provider

  Possible values: `disabled`, `subprocess`

* `--concurrent-solves <CONCURRENT_SOLVES>` — Max concurrent solves, default is the number of CPUs
* `--concurrent-downloads <CONCURRENT_DOWNLOADS>` — Max concurrent network requests, default is 50
* `--force-reinstall` — Specifies that the packages should be reinstalled even if they are already installed



## `pixi global uninstall`

Uninstalls environments from the global environment.

Example:
pixi global uninstall pixi-pack rattler-build

**Usage:** `pixi global uninstall [OPTIONS] <ENVIRONMENT>...`

###### **Arguments:**

* `<ENVIRONMENT>` — Specifies the environments that are to be removed

###### **Options:**

* `--tls-no-verify` — Do not verify the TLS certificate of the server
* `--auth-file <AUTH_FILE>` — Path to the file containing the authentication token
* `--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>` — Specifies if we want to use uv keyring provider

  Possible values: `disabled`, `subprocess`

* `--concurrent-solves <CONCURRENT_SOLVES>` — Max concurrent solves, default is the number of CPUs
* `--concurrent-downloads <CONCURRENT_DOWNLOADS>` — Max concurrent network requests, default is 50



## `pixi global remove`

Removes dependencies from an environment

Use `pixi global uninstall` to remove the whole environment

Example:
- pixi global remove --environment python numpy

**Usage:** `pixi global remove [OPTIONS] <PACKAGES>...`

###### **Arguments:**

* `<PACKAGES>` — Specifies the packages that are to be removed

###### **Options:**

* `-e`, `--environment <ENVIRONMENT>` — Specifies the environment that the dependencies need to be removed from
* `--tls-no-verify` — Do not verify the TLS certificate of the server
* `--auth-file <AUTH_FILE>` — Path to the file containing the authentication token
* `--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>` — Specifies if we want to use uv keyring provider

  Possible values: `disabled`, `subprocess`

* `--concurrent-solves <CONCURRENT_SOLVES>` — Max concurrent solves, default is the number of CPUs
* `--concurrent-downloads <CONCURRENT_DOWNLOADS>` — Max concurrent network requests, default is 50



## `pixi global list`

Lists all packages previously installed into a globally accessible location via `pixi global install`.

All environments:
- Yellow: the binaries that are exposed.
- Green: the packages that are explicit dependencies of the environment.
- Blue: the version of the installed package.
- Cyan: the name of the environment.

Per environment:
- Green: packages that are explicitly installed.

**Usage:** `pixi global list [OPTIONS] [REGEX]`

###### **Arguments:**

* `<REGEX>` — List only packages matching a regular expression. Without regex syntax it acts like a `contains` filter

###### **Options:**

* `--tls-no-verify` — Do not verify the TLS certificate of the server
* `--auth-file <AUTH_FILE>` — Path to the file containing the authentication token
* `--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>` — Specifies if we want to use uv keyring provider

  Possible values: `disabled`, `subprocess`

* `--concurrent-solves <CONCURRENT_SOLVES>` — Max concurrent solves, default is the number of CPUs
* `--concurrent-downloads <CONCURRENT_DOWNLOADS>` — Max concurrent network requests, default is 50
* `-e`, `--environment <ENVIRONMENT>` — The name of the environment to list
* `--sort-by <SORT_BY>` — Sorting strategy for the package table of an environment

  Default value: `name`

  Possible values: `size`, `name`




## `pixi global sync`

Sync global manifest with installed environments

**Usage:** `pixi global sync [OPTIONS]`

###### **Options:**

* `--tls-no-verify` — Do not verify the TLS certificate of the server
* `--auth-file <AUTH_FILE>` — Path to the file containing the authentication token
* `--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>` — Specifies if we want to use uv keyring provider

  Possible values: `disabled`, `subprocess`

* `--concurrent-solves <CONCURRENT_SOLVES>` — Max concurrent solves, default is the number of CPUs
* `--concurrent-downloads <CONCURRENT_DOWNLOADS>` — Max concurrent network requests, default is 50



## `pixi global expose`

Interact with the exposure of binaries in the global environment

`pixi global expose add python310=python3.10 --environment myenv` will expose the `python3.10` executable as `python310` from the environment `myenv`

`pixi global expose remove python310 --environment myenv` will remove the exposed name `python310` from the environment `myenv`

**Usage:** `pixi global expose <COMMAND>`

###### **Subcommands:**

* `add` — Add exposed binaries from an environment to your global environment
* `remove` — Remove exposed binaries from the global environment



## `pixi global expose add`

Add exposed binaries from an environment to your global environment

Example:
- pixi global expose add python310=python3.10 python3=python3 --environment myenv
- pixi global add --environment my_env pytest pytest-cov --expose pytest=pytest

**Usage:** `pixi global expose add [OPTIONS] --environment <ENVIRONMENT> [MAPPINGS]...`

###### **Arguments:**

* `<MAPPINGS>` — Add one or more mapping which describe which executables are exposed. The syntax is `exposed_name=executable_name`, so for example `python3.10=python`. Alternatively, you can input only an executable_name and `executable_name=executable_name` is assumed

###### **Options:**

* `-e`, `--environment <ENVIRONMENT>` — The environment to which the binaries should be exposed
* `--tls-no-verify` — Do not verify the TLS certificate of the server
* `--auth-file <AUTH_FILE>` — Path to the file containing the authentication token
* `--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>` — Specifies if we want to use uv keyring provider

  Possible values: `disabled`, `subprocess`

* `--concurrent-solves <CONCURRENT_SOLVES>` — Max concurrent solves, default is the number of CPUs
* `--concurrent-downloads <CONCURRENT_DOWNLOADS>` — Max concurrent network requests, default is 50



## `pixi global expose remove`

Remove exposed binaries from the global environment

`pixi global expose remove python310 python3 --environment myenv` will remove the exposed names `python310` and `python3` from the environment `myenv`

**Usage:** `pixi global expose remove [OPTIONS] [EXPOSED_NAMES]...`

###### **Arguments:**

* `<EXPOSED_NAMES>` — The exposed names that should be removed

###### **Options:**

* `--tls-no-verify` — Do not verify the TLS certificate of the server
* `--auth-file <AUTH_FILE>` — Path to the file containing the authentication token
* `--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>` — Specifies if we want to use uv keyring provider

  Possible values: `disabled`, `subprocess`

* `--concurrent-solves <CONCURRENT_SOLVES>` — Max concurrent solves, default is the number of CPUs
* `--concurrent-downloads <CONCURRENT_DOWNLOADS>` — Max concurrent network requests, default is 50



## `pixi global update`

Updates environments in the global environment

**Usage:** `pixi global update [OPTIONS] [ENVIRONMENTS]...`

###### **Arguments:**

* `<ENVIRONMENTS>` — Specifies the environments that are to be updated

###### **Options:**

* `--tls-no-verify` — Do not verify the TLS certificate of the server
* `--auth-file <AUTH_FILE>` — Path to the file containing the authentication token
* `--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>` — Specifies if we want to use uv keyring provider

  Possible values: `disabled`, `subprocess`

* `--concurrent-solves <CONCURRENT_SOLVES>` — Max concurrent solves, default is the number of CPUs
* `--concurrent-downloads <CONCURRENT_DOWNLOADS>` — Max concurrent network requests, default is 50



## `pixi auth`

Login to prefix.dev or anaconda.org servers to access private channels

**Usage:** `pixi auth <COMMAND>`

###### **Subcommands:**

* `login` — Store authentication information for a given host
* `logout` — Remove authentication information for a given host



## `pixi auth login`

Store authentication information for a given host

**Usage:** `pixi auth login [OPTIONS] <HOST>`

###### **Arguments:**

* `<HOST>` — The host to authenticate with (e.g. repo.prefix.dev)

###### **Options:**

* `--token <TOKEN>` — The token to use (for authentication with prefix.dev)
* `--username <USERNAME>` — The username to use (for basic HTTP authentication)
* `--password <PASSWORD>` — The password to use (for basic HTTP authentication)
* `--conda-token <CONDA_TOKEN>` — The token to use on anaconda.org / quetz authentication
* `--s3-access-key-id <S3_ACCESS_KEY_ID>` — The S3 access key ID
* `--s3-secret-access-key <S3_SECRET_ACCESS_KEY>` — The S3 secret access key
* `--s3-session-token <S3_SESSION_TOKEN>` — The S3 session token



## `pixi auth logout`

Remove authentication information for a given host

**Usage:** `pixi auth logout <HOST>`

###### **Arguments:**

* `<HOST>` — The host to remove authentication for



## `pixi config`

Configuration management

**Usage:** `pixi config <COMMAND>`

###### **Subcommands:**

* `edit` — Edit the configuration file
* `list` — List configuration values
* `prepend` — Prepend a value to a list configuration key
* `append` — Append a value to a list configuration key
* `set` — Set a configuration value
* `unset` — Unset a configuration value



## `pixi config edit`

Edit the configuration file

**Usage:** `pixi config edit [OPTIONS] [EDITOR]`

###### **Arguments:**

* `<EDITOR>` — The editor to use, defaults to `EDITOR` environment variable or `nano` on Unix and `notepad` on Windows

###### **Options:**

* `-l`, `--local` — Operation on project-local configuration
* `-g`, `--global` — Operation on global configuration
* `-s`, `--system` — Operation on system configuration
* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory



## `pixi config list`

List configuration values

Example: pixi config list default-channels

**Usage:** `pixi config list [OPTIONS] [KEY]`

###### **Arguments:**

* `<KEY>` — Configuration key to show (all if not provided)

###### **Options:**

* `--json` — Output in JSON format
* `-l`, `--local` — Operation on project-local configuration
* `-g`, `--global` — Operation on global configuration
* `-s`, `--system` — Operation on system configuration
* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory



## `pixi config prepend`

Prepend a value to a list configuration key

Example: pixi config prepend default-channels bioconda

**Usage:** `pixi config prepend [OPTIONS] <KEY> <VALUE>`

###### **Arguments:**

* `<KEY>` — Configuration key to set
* `<VALUE>` — Configuration value to (pre|ap)pend

###### **Options:**

* `-l`, `--local` — Operation on project-local configuration
* `-g`, `--global` — Operation on global configuration
* `-s`, `--system` — Operation on system configuration
* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory



## `pixi config append`

Append a value to a list configuration key

Example: pixi config append default-channels bioconda

**Usage:** `pixi config append [OPTIONS] <KEY> <VALUE>`

###### **Arguments:**

* `<KEY>` — Configuration key to set
* `<VALUE>` — Configuration value to (pre|ap)pend

###### **Options:**

* `-l`, `--local` — Operation on project-local configuration
* `-g`, `--global` — Operation on global configuration
* `-s`, `--system` — Operation on system configuration
* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory



## `pixi config set`

Set a configuration value

Example: pixi config set default-channels '["conda-forge", "bioconda"]'

**Usage:** `pixi config set [OPTIONS] <KEY> [VALUE]`

###### **Arguments:**

* `<KEY>` — Configuration key to set
* `<VALUE>` — Configuration value to set (key will be unset if value not provided)

###### **Options:**

* `-l`, `--local` — Operation on project-local configuration
* `-g`, `--global` — Operation on global configuration
* `-s`, `--system` — Operation on system configuration
* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory



## `pixi config unset`

Unset a configuration value

Example: pixi config unset default-channels

**Usage:** `pixi config unset [OPTIONS] <KEY>`

###### **Arguments:**

* `<KEY>` — Configuration key to unset

###### **Options:**

* `-l`, `--local` — Operation on project-local configuration
* `-g`, `--global` — Operation on global configuration
* `-s`, `--system` — Operation on system configuration
* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory



## `pixi info`

Information about the system, project and environments for the current machine

**Usage:** `pixi info [OPTIONS]`

###### **Options:**

* `--extended` — Show cache and environment size
* `--json` — Whether to show the output as JSON or not
* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory



## `pixi upload`

Upload a conda package

With this command, you can upload a conda package to a channel. Example: pixi upload https://prefix.dev/api/v1/upload/my_channel my_package.conda

Use `pixi auth login` to authenticate with the server.

**Usage:** `pixi upload <HOST> <PACKAGE_FILE>`

###### **Arguments:**

* `<HOST>` — The host + channel to upload to
* `<PACKAGE_FILE>` — The file to upload



## `pixi search`

Search a conda package

Its output will list the latest version of package.

**Usage:** `pixi search [OPTIONS] <PACKAGE>`

###### **Arguments:**

* `<PACKAGE>` — Name of package to search

###### **Options:**

* `-c`, `--channel <CHANNEL>` — The channels to consider as a name or a url. Multiple channels can be specified by using this field multiple times.

   When specifying a channel, it is common that the selected channel also depends on the `conda-forge` channel.

   By default, if no channel is provided, `conda-forge` is used.
* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory
* `-p`, `--platform <PLATFORM>` — The platform to search for, defaults to current platform

  Default value: `linux-64`
* `-l`, `--limit <LIMIT>` — Limit the number of search results



## `pixi clean`

Clean the parts of your system which are touched by pixi. Defaults to cleaning the environments and task cache. Use the `cache` subcommand to clean the cache

**Usage:** `pixi clean [OPTIONS] [COMMAND]`

###### **Subcommands:**

* `cache` — Clean the cache of your system which are touched by pixi

###### **Options:**

* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory
* `-e`, `--environment <ENVIRONMENT>` — The environment directory to remove
* `--activation-cache` — Only remove the activation cache



## `pixi clean cache`

Clean the cache of your system which are touched by pixi

**Usage:** `pixi clean cache [OPTIONS]`

###### **Options:**

* `--pypi` — Clean only the pypi related cache
* `--conda` — Clean only the conda related cache
* `--mapping` — Clean only the mapping cache
* `--exec` — Clean only `exec` cache
* `--repodata` — Clean only the repodata cache
* `--tool` — Clean only the build backend tools cache
* `-y`, `--yes` — Answer yes to all questions



## `pixi completion`

Generates a completion script for a shell

**Usage:** `pixi completion --shell <SHELL>`

###### **Options:**

* `-s`, `--shell <SHELL>` — The shell to generate a completion script for

  Possible values:
  - `bash`:
    Bourne Again SHell (bash)
  - `elvish`:
    Elvish shell
  - `fish`:
    Friendly Interactive SHell (fish)
  - `nushell`:
    Nushell
  - `powershell`:
    PowerShell
  - `zsh`:
    Z SHell (zsh)




## `pixi build`

Workspace configuration

**Usage:** `pixi build [OPTIONS]`

###### **Options:**

* `--manifest-path <MANIFEST_PATH>` — The path to `pixi.toml`, `pyproject.toml`, or the project directory
* `--tls-no-verify` — Do not verify the TLS certificate of the server
* `--auth-file <AUTH_FILE>` — Path to the file containing the authentication token
* `--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>` — Specifies if we want to use uv keyring provider

  Possible values: `disabled`, `subprocess`

* `--concurrent-solves <CONCURRENT_SOLVES>` — Max concurrent solves, default is the number of CPUs
* `--concurrent-downloads <CONCURRENT_DOWNLOADS>` — Max concurrent network requests, default is 50
* `-t`, `--target-platform <TARGET_PLATFORM>` — The target platform to build for (defaults to the current platform)

  Default value: `linux-64`
* `-o`, `--output-dir <OUTPUT_DIR>` — The output directory to place the build artifacts

  Default value: `.`
