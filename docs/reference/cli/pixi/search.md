<!--- This file is autogenerated. Do not edit manually! -->
# <code>[pixi](../pixi.md) search</code>

## About
Search a conda package

--8<-- "docs/reference/cli/pixi/search_extender:description"

## Usage
```
pixi search [OPTIONS] <PACKAGE>
```

## Arguments
- <a id="arg-<PACKAGE>" href="#arg-<PACKAGE>">`<PACKAGE>`</a>
:  Name of package to search
<br>**required**: `true`

## Options
- <a id="arg---channel" href="#arg---channel">`--channel (-c) <CHANNEL>`</a>
:  The channels to consider as a name or a url. Multiple channels can be specified by using this field multiple times
<br>May be provided more than once.
- <a id="arg---platform" href="#arg---platform">`--platform (-p) <PLATFORM>`</a>
:  The platform to search for, defaults to current platform
<br>**default**: `current_platform`
- <a id="arg---limit" href="#arg---limit">`--limit (-l) <LIMIT>`</a>
:  Limit the number of search results

## Global Options
- <a id="arg---manifest-path" href="#arg---manifest-path">`--manifest-path <MANIFEST_PATH>`</a>
:  The path to `pixi.toml`, `pyproject.toml`, or the workspace directory

## Description
Search a conda package

Its output will list the latest version of package.


--8<-- "docs/reference/cli/pixi/search_extender:example"
