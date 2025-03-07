# <code>[pixi](../pixi.md) project</code>

## About
Modify the project configuration file through the command line

--8<-- "docs/reference/cli/pixi/project_extender.md:description"

## Usage
```
pixi project [OPTIONS] <COMMAND>
```

## Subcommands
| Command | Description |
|---------|-------------|
| [`channel`](channel) | Commands to manage project channels |
| [`description`](description) | Commands to manage project description |
| [`platform`](platform) | Commands to manage project platforms |
| [`version`](version) | Commands to manage project version |
| [`environment`](environment) | Commands to manage project environments |
| [`export`](export) | Commands to export projects to other formats |
| [`name`](name) | Commands to manage project name |
| [`system-requirements`](system-requirements) | Commands to manage project environments |


## Global Options
- <a id="arg---manifest-path" href="#arg---manifest-path">`--manifest-path <MANIFEST_PATH>`</a>
:  The path to `pixi.toml`, `pyproject.toml`, or the project directory

--8<-- "docs/reference/cli/pixi/project_extender.md:example"
