# <code>[pixi](../../pixi.md) [global](../global.md) upgrade</code>

## About
Upgrade specific package which is installed globally. This command has been removed, please use `pixi global update` instead

--8<-- "docs/reference/cli/pixi/global/upgrade_extender.md:description"

## Usage
```
pixi global upgrade [OPTIONS] [SPECS]...
```

## Arguments
- <a id="arg-<SPECS>" href="#arg-<SPECS>">`<SPECS>`</a>
:  Specifies the packages to upgrade

## Options
- <a id="arg---channel" href="#arg---channel">`--channel (-c) <CHANNEL>`</a>
:  The channels to consider as a name or a url. Multiple channels can be specified by using this field multiple times
- <a id="arg---platform" href="#arg---platform">`--platform <PLATFORM>`</a>
:  The platform to install the package for
<br>**default**: `osx-arm64`

--8<-- "docs/reference/cli/pixi/global/upgrade_extender.md:example"
