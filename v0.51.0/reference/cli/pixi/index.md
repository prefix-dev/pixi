# `pixi`

## Description

The `pixi` command is the main entry point for the Pixi CLI.

## Usage

```text
pixi [OPTIONS] <COMMAND>

```

## Subcommands

| Command                       | Description                                                                                                                               |
| ----------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| [`add`](add/)                 | Adds dependencies to the workspace                                                                                                        |
| [`auth`](auth/)               | Login to prefix.dev or anaconda.org servers to access private channels                                                                    |
| [`build`](build/)             | Workspace configuration                                                                                                                   |
| [`clean`](clean/)             | Cleanup the environments                                                                                                                  |
| [`completion`](completion/)   | Generates a completion script for a shell                                                                                                 |
| [`config`](config/)           | Configuration management                                                                                                                  |
| [`exec`](exec/)               | Run a command and install it in a temporary environment                                                                                   |
| [`global`](global/)           | Subcommand for global package management actions                                                                                          |
| [`info`](info/)               | Information about the system, workspace and environments for the current machine                                                          |
| [`init`](init/)               | Creates a new workspace                                                                                                                   |
| [`import`](import/)           | Imports a file into an environment in an existing workspace.                                                                              |
| [`install`](install/)         | Install an environment, both updating the lockfile and installing the environment                                                         |
| [`list`](list/)               | List workspace's packages                                                                                                                 |
| [`lock`](lock/)               | Solve environment and update the lock file without installing the environments                                                            |
| [`reinstall`](reinstall/)     | Re-install an environment, both updating the lockfile and re-installing the environment                                                   |
| [`remove`](remove/)           | Removes dependencies from the workspace                                                                                                   |
| [`run`](run/)                 | Runs task in the pixi environment                                                                                                         |
| [`search`](search/)           | Search a conda package                                                                                                                    |
| [`self-update`](self-update/) | Update pixi to the latest version or a specific version                                                                                   |
| [`shell`](shell/)             | Start a shell in a pixi environment, run `exit` to leave the shell                                                                        |
| [`shell-hook`](shell-hook/)   | Print the pixi environment activation script                                                                                              |
| [`task`](task/)               | Interact with tasks in the workspace                                                                                                      |
| [`tree`](tree/)               | Show a tree of workspace dependencies                                                                                                     |
| [`update`](update/)           | The `update` command checks if there are newer versions of the dependencies and updates the `pixi.lock` file and environments accordingly |
| [`upgrade`](upgrade/)         | Checks if there are newer versions of the dependencies and upgrades them in the lockfile and manifest file                                |
| [`upload`](upload/)           | Upload a conda package                                                                                                                    |
| [`workspace`](workspace/)     | Modify the workspace configuration file through the command line                                                                          |

## Global Options

- [`--help (-h)`](#arg---help) Display help information

- [`--verbose (-v)`](#arg---verbose) Increase logging verbosity (-v for warnings, -vv for info, -vvv for debug, -vvvv for trace)

- [`--quiet (-q)`](#arg---quiet) Decrease logging verbosity (quiet mode)

- [`--color <COLOR>`](#arg---color) Whether the log needs to be colored

  **env**: `PIXI_COLOR`

  **default**: `auto`

  **options**: `always`, `never`, `auto`

- [`--no-progress`](#arg---no-progress) Hide all progress bars, always turned on if stderr is not a terminal

  **env**: `PIXI_NO_PROGRESS`

  **default**: `false`
