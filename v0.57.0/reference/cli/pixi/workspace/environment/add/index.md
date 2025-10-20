# `[pixi](../../../) [workspace](../../) [environment](../) add`

## About

Adds an environment to the manifest file

## Usage

```text
pixi workspace environment add [OPTIONS] <NAME>

```

## Arguments

- [`<NAME>`](#arg-%3CNAME%3E) The name of the environment to add

  **required**: `true`

## Options

- [`--feature (-f) <FEATURES>`](#arg---feature) Features to add to the environment

  May be provided more than once.

- [`--solve-group <SOLVE_GROUP>`](#arg---solve-group) The solve-group to add the environment to

- [`--no-default-feature`](#arg---no-default-feature) Don't include the default feature in the environment

  **default**: `false`

- [`--force`](#arg---force) Update the manifest even if the environment already exists

  **default**: `false`
