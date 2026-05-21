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

- [`--platform (-p) <PLATFORM>`](#arg---platform) : The platform(s) to search for. By default, searches all platforms from the manifest (or all known platforms if no manifest is found)

- [`--limit (-l) <LIMIT>`](#arg---limit) : Limit the number of versions shown per package, -1 for no limit

  ```
  **default**: `5`
  ```

- [`--limit-packages (-n) <LIMIT_PACKAGES>`](#arg---limit-packages) : Limit the number of packages shown, -1 for no limit

  ```
  **default**: `5`
  ```

- [`--json`](#arg---json) : Output in JSON format

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
