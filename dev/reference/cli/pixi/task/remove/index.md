# `[pixi](../../) [task](../) remove`

## About

Remove a command from the workspace

## Usage

```text
pixi task remove [OPTIONS] [TASK_NAME]...

```

## Arguments

- [`<TASK_NAME>`](#arg-%3CTASK_NAME%3E) Task name to remove

  May be provided more than once.

## Options

- [`--platform (-p) <PLATFORM>`](#arg---platform) The platform for which the task should be removed
- [`--feature (-f) <FEATURE>`](#arg---feature) The feature for which the task should be removed

## Examples

```shell
pixi task remove cow
pixi task remove --platform linux-64 test
pixi task remove --feature cuda task

```
