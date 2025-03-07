# <code>[pixi](../pixi.md) clean</code>

## About
Clean the parts of your system which are touched by pixi. Defaults to cleaning the environments and task cache. Use the `cache` subcommand to clean the cache

--8<-- "docs/reference/cli/pixi/clean_extender.md:description"

## Usage
```
pixi clean [OPTIONS] [COMMAND]
```

## Subcommands
| Command | Description |
|---------|-------------|
| [`cache`](clean/cache.md) | Clean the cache of your system which are touched by pixi |


## Options
- <a id="arg---activation-cache" href="#arg---activation-cache">`--activation-cache`</a>
:  Only remove the activation cache
- <a id="arg---environment" href="#arg---environment">`--environment (-e) <ENVIRONMENT>`</a>
:  The environment directory to remove

## Global Options
- <a id="arg---manifest-path" href="#arg---manifest-path">`--manifest-path <MANIFEST_PATH>`</a>
:  The path to `pixi.toml`, `pyproject.toml`, or the project directory

--8<-- "docs/reference/cli/pixi/clean_extender.md:example"
