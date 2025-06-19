# Basic usage of Pixi

Pixi can do alot of things, but it is designed to be simple to use.
Let's go through the basic usage of Pixi.

## Managing workspaces or projects

- [`pixi init`](./reference/cli/pixi/init.md) - create a new Pixi manifest in the current directory
- [`pixi add`](./reference/cli/pixi/add.md) - add a dependency to your manifest
- [`pixi remove`](./reference/cli/pixi/remove.md) - remove a dependency from your manifest
- [`pixi update`](./reference/cli/pixi/update.md) - update dependencies in your manifest
- [`pixi upgrade`](./reference/cli/pixi/upgrade.md) - upgrade the dependencies in your manifest to the latest versions, even if you pinned them to a specific version
- [`pixi lock`](./reference/cli/pixi/lock.md) - create or update the lockfile for your manifest
- [`pixi info`](./reference/cli/pixi/info.md) - show information about your workspace
- [`pixi run`](./reference/cli/pixi/run.md) - run a task defined in your manifest or any command in the current environment
- [`pixi shell`](./reference/cli/pixi/shell.md) - start a shell in the current environment
- [`pixi list`](./reference/cli/pixi/list.md) - list all dependencies in the current environment
- [`pixi tree`](./reference/cli/pixi/tree.md) - show a tree of dependencies in the current environment
- [`pixi clean`](./reference/cli/pixi/clean.md) - remove the environment from your machine

## Managing global installations
Pixi can manage global installations of tools and environments.
It installs the environments in a central location, so you can use them from anywhere.

- [`pixi global install`](./reference/cli/pixi/global/install.md) - install a package into it's own environment in the global space.
- [`pixi global uninstall`](./reference/cli/pixi/global/uninstall.md) - uninstall an environment from the global space.
- [`pixi global add`](./reference/cli/pixi/global/add.md) - add a package to an existing globally installed environment.
- [`pixi global sync`](./reference/cli/pixi/global/sync.md) - sync the globally installed environments with the global manifest, describing all the environments you want to install.
- [`pixi global edit`](./reference/cli/pixi/global/edit.md) - edit the global manifest.
- [`pixi global update`](./reference/cli/pixi/global/update.md) - update the global environments
- [`pixi global list`](./reference/cli/pixi/global/list.md) - list all the installed environments

More information: [Global Tools](./global_tools/introduction.md)

## Running one-off commands
Pixi can run one-off commands in a specific environment.

- [`pixi exec`](./reference/cli/pixi/exec.md) - run a command in a temporary environment.
- [`pixi exec --spec`](./reference/cli/pixi/exec.md#arg---spec)   - run a command in a temporary environment, with a specific specification.

For example:

```bash
> pixi exec python -VV
Python 3.13.5 | packaged by conda-forge | (main, Jun 16 2025, 08:24:05) [Clang 18.1.8 ]
> pixi exec --spec "python=3.12" python -VV
Python 3.12.11 | packaged by conda-forge | (main, Jun  4 2025, 14:38:53) [Clang 18.1.8 ]
```

## Multiple environments
Pixi workspaces allow you to manage multiple environments.
An environment is build out of one or multiple features.

- [`pixi add --feature`](./reference/cli/pixi/add.md#arg---feature) - add a package to a feature
- [`pixi task add --feature`](./reference/cli/pixi/task/add.md#arg---feature) - add a task to a specific feature
- [`pixi workspace environment add`](./reference/cli/pixi/workspace/environment/add.md) - add an environment to the workspace
- [`pixi run --environment`](./reference/cli/pixi/run.md#arg---environment) - run a command in a specific environment
- [`pixi shell --environment`](./reference/cli/pixi/shell.md#arg---environment) - activate a specific environment
- [`pixi list --environment`](./reference/cli/pixi/list.md#arg---environment) - list the dependencies in a specific environment

More information: [Multiple environments](./workspace/multi_environment.md)

## Tasks
Pixi can run cross-platform tasks using it's built-in task runner.
This can be a predefined task or any normal executable.

- [`pixi run`](./reference/cli/pixi/run.md) - Run a task or command
- [`pixi task add`](./reference/cli/pixi/task/add.md) - Add a new task to the manifest

Tasks can have other tasks as dependencies.
Here is an example of a more complex task usecase
```toml title="pixi.toml"
[tasks]
build = "make build"
# using the toml table view
[tasks.test]
cmd = "pytest"
depends-on = ["build"]
```
More information: [Tasks](./workspace/advanced_tasks.md)

## Multi platform support
Pixi supports multiple platforms out of the box.
You can specify which platforms your workspace supports and Pixi will ensure that the dependencies are compatible with those platforms.

- [`pixi add --platform`](./reference/cli/pixi/add.md#arg---platform) - add a package only to a specific platform
- [`pixi workspace platform add`](./reference/cli/pixi/workspace/platform/add.md) - add a platform to the workspace that you want to support with your project

More information: [Multi platform support](./workspace/multi_platform.md)

## Utilities
Pixi comes with a set of utilities to help you debug or manage your setup.

- [`pixi info`](./reference/cli/pixi/info.md) - Show information about the current workspace, and the global setup.
- [`pixi config`](./reference/cli/pixi/config.md) - Show or edit the Pixi configuration.
- [`pixi tree`](./reference/cli/pixi/tree.md) - Show a tree of dependencies in the current environment.
- [`pixi list`](./reference/cli/pixi/list.md) - List all dependencies in the current environment.
- [`pixi clean`](./reference/cli/pixi/clean.md) - Remove the project environments from your machine.
- `pixi help` - Show help for Pixi commands.
- `pixi help <subcommand>` - Show help for a specific Pixi command.
- [`pixi auth`](./reference/cli/pixi/auth.md) - Manage authentication for conda channels.
- [`pixi search`](./reference/cli/pixi/search.md) - Search for packages in the configured channels.
- [`pixi completion`](./reference/cli/pixi/completion.md) - Generate shell completion scripts for Pixi commands.


## Going further

There is still much more that Pixi has to offer.
Check out the topics on the sidebar on the left to learn more.

And don't forget to [join our Discord](https://discord.gg/kKV8ZxyzY4) to join our community of Pixi enthusiasts!
