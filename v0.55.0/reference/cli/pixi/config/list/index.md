# `[pixi](../../) [config](../) list`

## About

List configuration values

## Usage

```text
pixi config list [OPTIONS] [KEY]

```

## Arguments

- [`<KEY>`](#arg-%3CKEY%3E) Configuration key to show (all if not provided)

## Options

- [`--json`](#arg---json) Output in JSON format

## Config Options

- [`--local (-l)`](#arg---local) Operation on project-local configuration
- [`--global (-g)`](#arg---global) Operation on global configuration
- [`--system (-s)`](#arg---system) Operation on system configuration

## Global Options

- [`--manifest-path <MANIFEST_PATH>`](#arg---manifest-path) The path to `pixi.toml`, `pyproject.toml`, or the workspace directory

## Description

List configuration values

Example: `pixi config list default-channels`

## Examples

```shell
pixi config list default-channels
pixi config list --json
pixi config list --system
pixi config list -g

```
