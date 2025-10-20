# `[pixi](../../) [config](../) set`

## About

Set a configuration value

## Usage

```text
pixi config set [OPTIONS] <KEY> [VALUE]

```

## Arguments

- [`<KEY>`](#arg-%3CKEY%3E) Configuration key to set

  **required**: `true`

- [`<VALUE>`](#arg-%3CVALUE%3E) Configuration value to set (key will be unset if value not provided)

## Config Options

- [`--local (-l)`](#arg---local) Operation on project-local configuration
- [`--global (-g)`](#arg---global) Operation on global configuration
- [`--system (-s)`](#arg---system) Operation on system configuration

## Global Options

- [`--manifest-path <MANIFEST_PATH>`](#arg---manifest-path) The path to `pixi.toml`, `pyproject.toml`, or the workspace directory

## Description

Set a configuration value

Example: `pixi config set default-channels '["conda-forge", "bioconda"]'`

## Examples

```shell
pixi config set default-channels '["conda-forge", "bioconda"]'
pixi config set --global mirrors '{"https://conda.anaconda.org/conda-forge": ["https://prefix.dev/conda-forge"]}'
pixi config set repodata-config.disable-zstd true --system
pixi config set --global detached-environments "/opt/pixi/envs"
pixi config set detached-environments false
pixi config set s3-options.my-bucket '{"endpoint-url": "http://localhost:9000", "force-path-style": true, "region": "auto"}'

```
