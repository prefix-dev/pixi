# [pixi](../) search

Search a conda package

## Usage

```text
pixi search [OPTIONS] <PACKAGE>
```

## Arguments

- [`<PACKAGE>`](#arg-%3CPACKAGE%3E) : MatchSpec of a package to search

  ```
  **required**: `true`
  ```

## Options

- [`--channel (-c) <CHANNEL>`](#arg---channel) : The channels to consider as a name or a url. Multiple channels can be specified by using this field multiple times

  ```
  May be provided more than once.
  ```

- [`--platform (-p) <PLATFORM>`](#arg---platform) : The platform to search packages for. By default, searches all platforms from the manifest (or all known platforms if no manifest is found). Accepts a workspace platform name; a bare conda subdir (e.g. `linux-64`) is also accepted

- [`--limit (-l) <LIMIT>`](#arg---limit) : Limit the number of versions shown per package, -1 for no limit

  ```
  **default**: `5`
  ```

- [`--limit-packages (-n) <LIMIT_PACKAGES>`](#arg---limit-packages) : Limit the number of packages shown, -1 for no limit

  ```
  **default**: `5`
  ```

- [`--json`](#arg---json) : Output in JSON format

## Config Options

- [`--no-config`](#arg---no-config) : Don't read system or user-level configuration files. Project-local `<project>/.pixi/config.toml` is still loaded

  ```
  **env**: `PIXI_NO_CONFIG`
    
  **default**: `false`
  ```

- [`--config-file <PATH>`](#arg---config-file) : Load configuration from this file instead of searching system and user-level paths. Project-local `<project>/.pixi/config.toml` is still merged on top

  ```
  **env**: `PIXI_CONFIG_FILE`
  ```

## Global Options

- [`--manifest-path (-m) <MANIFEST_PATH>`](#arg---manifest-path) : The path to `pixi.toml`, `pyproject.toml`, or the workspace directory
- [`--workspace (-w) <WORKSPACE>`](#arg---workspace) : Name of the workspace

## Description

Search a conda package

Its output will list the latest version of package.

## Examples

```shell
pixi search pixi
pixi search --limit 30 "py*"
# search in a different channel and for a specific platform
pixi search -c robostack --platform linux-64 "*plotjuggler*"
# search for a specific version of a package
pixi search "rattler-build<=0.35.4"
pixi search "rattler-build[build_number=h2d22210_0]" --platform linux-64
```
