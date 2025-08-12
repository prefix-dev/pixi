# `[pixi](../../) [task](../) list`

## About

List all tasks in the workspace

## Usage

```text
pixi task list [OPTIONS]

```

## Options

- [`--summary (-s)`](#arg---summary) Tasks available for this machine per environment
- [`--environment (-e) <ENVIRONMENT>`](#arg---environment) The environment the list should be generated for. If not specified, the default environment is used
- [`--json`](#arg---json) List as json instead of a tree If not specified, the default environment is used

## Examples

```shell
pixi task list
pixi task list --environment cuda
pixi task list --summary

```
