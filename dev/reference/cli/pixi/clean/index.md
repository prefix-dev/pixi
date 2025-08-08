# `[pixi](../) clean`

## About

Cleanup the environments

## Usage

```text
pixi clean [OPTIONS] [COMMAND]

```

## Subcommands

| Command           | Description                                              |
| ----------------- | -------------------------------------------------------- |
| [`cache`](cache/) | Clean the cache of your system which are touched by pixi |

## Options

- [`--environment (-e) <ENVIRONMENT>`](#arg---environment) The environment directory to remove
- [`--activation-cache`](#arg---activation-cache) Only remove the activation cache
- [`--build`](#arg---build) Only remove the pixi-build cache

## Global Options

- [`--manifest-path <MANIFEST_PATH>`](#arg---manifest-path) The path to `pixi.toml`, `pyproject.toml`, or the workspace directory

## Description

Cleanup the environments.

This command removes the information in the .pixi folder. You can specify the environment to remove with the `--environment` flag.

Use the `cache` subcommand to clean the cache.
