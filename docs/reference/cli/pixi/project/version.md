# <code>[pixi](../../pixi.md) [project](../project.md) version</code>

## About
Commands to manage project version

--8<-- "docs/reference/cli/pixi/project/version_extender.md:description"

## Usage
```
pixi project version [OPTIONS] <COMMAND>
```

## Subcommands
| Command | Description |
|---------|-------------|
| [`get`](get) | Get the workspace version |
| [`set`](set) | Set the workspace version |
| [`major`](major) | Bump the workspace version to MAJOR |
| [`minor`](minor) | Bump the workspace version to MINOR |
| [`patch`](patch) | Bump the workspace version to PATCH |


## Global Options
- <a id="arg---manifest-path" href="#arg---manifest-path">`--manifest-path <MANIFEST_PATH>`</a>
:  The path to `pixi.toml`, `pyproject.toml`, or the project directory

--8<-- "docs/reference/cli/pixi/project/version_extender.md:example"
