# `[pixi](../../) [config](../) prepend`

## About

Prepend a value to a list configuration key

## Usage

```text
pixi config prepend [OPTIONS] <KEY> <VALUE>

```

## Arguments

- [`<KEY>`](#arg-%3CKEY%3E) Configuration key to set

  **required**: `true`

- [`<VALUE>`](#arg-%3CVALUE%3E) Configuration value to (pre|ap)pend

  **required**: `true`

## Config Options

- [`--local (-l)`](#arg---local) Operation on project-local configuration
- [`--global (-g)`](#arg---global) Operation on global configuration
- [`--system (-s)`](#arg---system) Operation on system configuration

## Global Options

- [`--manifest-path <MANIFEST_PATH>`](#arg---manifest-path) The path to `pixi.toml`, `pyproject.toml`, or the workspace directory

## Description

Prepend a value to a list configuration key

Example: `pixi config prepend default-channels bioconda`

## Examples

```shell
pixi config prepend default-channels conda-forge

```
