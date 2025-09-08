# `[pixi](../../../) [workspace](../../) [export](../) conda-environment`

## About

Export workspace environment to a conda environment.yaml file

## Usage

```text
pixi workspace export conda-environment [OPTIONS] [OUTPUT_PATH]

```

## Arguments

- [`<OUTPUT_PATH>`](#arg-%3COUTPUT_PATH%3E) Explicit path to export the environment file to

## Options

- [`--platform (-p) <PLATFORM>`](#arg---platform) The platform to render the environment file for. Defaults to the current platform
- [`--environment (-e) <ENVIRONMENT>`](#arg---environment) The environment to render the environment file for. Defaults to the default environment

## Global Options

- [`--manifest-path <MANIFEST_PATH>`](#arg---manifest-path) The path to `pixi.toml`, `pyproject.toml`, or the workspace directory
