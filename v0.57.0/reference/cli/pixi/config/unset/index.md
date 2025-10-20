# `[pixi](../../) [config](../) unset`

## About

Unset a configuration value

## Usage

```text
pixi config unset [OPTIONS] <KEY>

```

## Arguments

- [`<KEY>`](#arg-%3CKEY%3E) Configuration key to unset

  **required**: `true`

## Config Options

- [`--local (-l)`](#arg---local) Operation on project-local configuration
- [`--global (-g)`](#arg---global) Operation on global configuration
- [`--system (-s)`](#arg---system) Operation on system configuration

## Global Options

- [`--manifest-path <MANIFEST_PATH>`](#arg---manifest-path) The path to `pixi.toml`, `pyproject.toml`, or the workspace directory

## Description

Unset a configuration value

Example: `pixi config unset default-channels`

## Examples

```shell
pixi config unset default-channels
pixi config unset --global mirrors
pixi config unset repodata-config.disable-zstd --system

```
