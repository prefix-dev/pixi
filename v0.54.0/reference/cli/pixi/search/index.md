# `[pixi](../) search`

## About

Search a conda package

## Usage

```text
pixi search [OPTIONS] <PACKAGE>

```

## Arguments

- [`<PACKAGE>`](#arg-%3CPACKAGE%3E) Name of package to search

  **required**: `true`

## Options

- [`--channel (-c) <CHANNEL>`](#arg---channel) The channels to consider as a name or a url. Multiple channels can be specified by using this field multiple times

  May be provided more than once.

- [`--platform (-p) <PLATFORM>`](#arg---platform) The platform to search for, defaults to current platform

  **default**: `current_platform`

- [`--limit (-l) <LIMIT>`](#arg---limit) Limit the number of search results

## Global Options

- [`--manifest-path <MANIFEST_PATH>`](#arg---manifest-path) The path to `pixi.toml`, `pyproject.toml`, or the workspace directory

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
