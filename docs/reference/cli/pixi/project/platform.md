# <code>[pixi](../../pixi.md) [project](../project.md) platform</code>

## About
Commands to manage project platforms

--8<-- "docs/reference/cli/pixi/project/platform_extender.md:description"

## Usage
```
pixi project platform [OPTIONS] <COMMAND>
```

## Subcommands
| Command | Description |
|---------|-------------|
| [`add`](platform/add.md) | Adds a platform(s) to the project file and updates the lockfile |
| [`list`](platform/list.md) | List the platforms in the project file |
| [`remove`](platform/remove.md) | Remove platform(s) from the project file and updates the lockfile |


## Global Options
- <a id="arg---manifest-path" href="#arg---manifest-path">`--manifest-path <MANIFEST_PATH>`</a>
:  The path to `pixi.toml`, `pyproject.toml`, or the project directory

--8<-- "docs/reference/cli/pixi/project/platform_extender.md:example"
