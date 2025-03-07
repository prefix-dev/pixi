# <code>[pixi](../pixi.md) task</code>

## About
Interact with tasks in the project

--8<-- "docs/reference/cli/pixi/task_extender.md:description"

## Usage
```
pixi task [OPTIONS] <COMMAND>
```

## Subcommands
| Command | Description |
|---------|-------------|
| [`add`](add) | Add a command to the project |
| [`remove`](remove) | Remove a command from the project |
| [`alias`](alias) | Alias another specific command |
| [`list`](list) | List all tasks in the project |


## Global Options
- <a id="arg---manifest-path" href="#arg---manifest-path">`--manifest-path <MANIFEST_PATH>`</a>
:  The path to `pixi.toml`, `pyproject.toml`, or the project directory

--8<-- "docs/reference/cli/pixi/task_extender.md:example"
