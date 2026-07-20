# [pixi](../../../) [workspace](../../) [export](../) conda-environment

Export workspace environment to a conda environment.yaml file

## Usage

```text
pixi workspace export conda-environment [OPTIONS] [OUTPUT_PATH]
```

## Arguments

- [`<OUTPUT_PATH>`](#arg-%3COUTPUT_PATH%3E) : Explicit path to export the environment file to

## Options

- [`--platform (-p) <PLATFORM>`](#arg---platform) : The platform to render the environment file for. Defaults to the current platform
- [`--environment (-e) <ENVIRONMENT>`](#arg---environment) : The environment to render the environment file for. Defaults to the default environment
- [`--name (-n) <NAME>`](#arg---name) : The name to use for the rendered conda environment. Defaults to the environment name
- [`--no-pypi`](#arg---no-pypi) : Exclude pypi dependencies from the exported environment file
- [`--from-lock-file`](#arg---from-lock-file) : Render the environment with packages pinned to the versions resolved in the lock file instead of the manifest specs

## Config Options

- [`--no-config`](#arg---no-config) : Don't read system or user-level configuration files. Project-local `<project>/.pixi/config.toml` is still loaded

  ```
  **env**: `PIXI_NO_CONFIG`
    
  **default**: `false`
  ```

- [`--config-file <PATH>`](#arg---config-file) : Load configuration from this file instead of searching system and user-level paths. Project-local `<project>/.pixi/config.toml` is still merged on top

  ```
  **env**: `PIXI_CONFIG_FILE`
  ```

## Global Options

- [`--manifest-path (-m) <MANIFEST_PATH>`](#arg---manifest-path) : The path to `pixi.toml`, `pyproject.toml`, or the workspace directory
- [`--workspace (-w) <WORKSPACE>`](#arg---workspace) : Name of the workspace
