# <code>[pixi](../pixi.md) global</code>

## About
Subcommand for global package management actions

--8<-- "docs/reference/cli/pixi/global_extender.md:description"

## Usage
```
pixi global <COMMAND>
```

## Subcommands
| Command | Description |
|---------|-------------|
| [`add`](add) | Adds dependencies to an environment |
| [`edit`](edit) | Edit the global manifest file |
| [`install`](install) | Installs the defined packages in a globally accessible location and exposes their command line applications. |
| [`uninstall`](uninstall) | Uninstalls environments from the global environment. |
| [`remove`](remove) | Removes dependencies from an environment |
| [`list`](list) | Lists all packages previously installed into a globally accessible location via `pixi global install`. |
| [`sync`](sync) | Sync global manifest with installed environments |
| [`expose`](expose) | Interact with the exposure of binaries in the global environment |
| [`update`](update) | Updates environments in the global environment |


## Description
Subcommand for global package management actions.

Install packages on the user level. Into to the [`$PIXI_HOME`] directory, which defaults to `~/.pixi`.


--8<-- "docs/reference/cli/pixi/global_extender.md:example"
