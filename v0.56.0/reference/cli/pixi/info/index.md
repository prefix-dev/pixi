# `[pixi](../) info`

## About

Information about the system, workspace and environments for the current machine

More information [here](../../../../advanced/explain_info_command/).

## Usage

```text
pixi info [OPTIONS]

```

## Options

- [`--extended`](#arg---extended) Show cache and environment size
- [`--json`](#arg---json) Whether to show the output as JSON or not

## Global Options

- [`--manifest-path <MANIFEST_PATH>`](#arg---manifest-path) The path to `pixi.toml`, `pyproject.toml`, or the workspace directory

## Examples

```shell
pixi info
pixi info --json
pixi info --extended

```
