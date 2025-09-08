# `[pixi](../../) [config](../) edit`

## About

Edit the configuration file

## Usage

```text
pixi config edit [OPTIONS] [EDITOR]

```

## Arguments

- [`<EDITOR>`](#arg-%3CEDITOR%3E) The editor to use, defaults to `EDITOR` environment variable or `nano` on Unix and `notepad` on Windows

  **env**: `EDITOR`

## Config Options

- [`--local (-l)`](#arg---local) Operation on project-local configuration
- [`--global (-g)`](#arg---global) Operation on global configuration
- [`--system (-s)`](#arg---system) Operation on system configuration

## Global Options

- [`--manifest-path <MANIFEST_PATH>`](#arg---manifest-path) The path to `pixi.toml`, `pyproject.toml`, or the workspace directory

## Examples

```shell
pixi config edit --system
pixi config edit --local
pixi config edit -g
pixi config edit --global code
pixi config edit --system vim

```
