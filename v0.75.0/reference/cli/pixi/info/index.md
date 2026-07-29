# [pixi](../) info

Information about the system, workspace and environments for the current machine

More information [here](../../../../advanced/explain_info_command/).

## Usage

```text
pixi info [OPTIONS]
```

## Options

- [`--extended`](#arg---extended) : Show cache and environment size
- [`--json`](#arg---json) : Whether to show the output as JSON or not

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

## Examples

```shell
pixi info
pixi info --json
pixi info --extended
```
