# `[pixi](../) global`

## About

Subcommand for global package management actions

All commands in this section are used to manage global installations of packages and environments through the global manifest. More info on the global manifest can be found [here](../../../../global_tools/introduction/).

Tip

Binaries and environments installed globally are stored in `~/.pixi` by default, this can be changed by setting the `PIXI_HOME` environment variable.

## Usage

```text
pixi global <COMMAND>

```

## Subcommands

| Command                   | Description                                                                                                                                                                   |
| ------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| [`add`](add/)             | Adds dependencies to an environment                                                                                                                                           |
| [`edit`](edit/)           | Edit the global manifest file                                                                                                                                                 |
| [`install`](install/)     | Installs the defined packages in a globally accessible location and exposes their command line applications.                                                                  |
| [`uninstall`](uninstall/) | Uninstalls environments from the global environment.                                                                                                                          |
| [`remove`](remove/)       | Removes dependencies from an environment                                                                                                                                      |
| [`list`](list/)           | Lists global environments with their dependencies and exposed commands. Can also display all packages within a specific global environment when using the --environment flag. |
| [`sync`](sync/)           | Sync global manifest with installed environments                                                                                                                              |
| [`expose`](expose/)       | Interact with the exposure of binaries in the global environment                                                                                                              |
| [`shortcut`](shortcut/)   | Interact with the shortcuts on your machine                                                                                                                                   |
| [`update`](update/)       | Updates environments in the global environment                                                                                                                                |
| [`tree`](tree/)           | Show a tree of dependencies for a specific global environment                                                                                                                 |

## Description

Subcommand for global package management actions.

Install packages on the user level. Into to the \[`$PIXI_HOME`\] directory, which defaults to `~/.pixi`.
