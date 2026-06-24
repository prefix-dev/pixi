# [pixi](../../) [workspace](../) platform

Commands to manage workspace platforms

## Usage

```text
pixi workspace platform [OPTIONS] <COMMAND>
```

## Subcommands

| Command             | Description                                                                                            |
| ------------------- | ------------------------------------------------------------------------------------------------------ |
| [`add`](add/)       | Adds a platform(s) to the workspace file and updates the lock file                                     |
| [`edit`](edit/)     | Edit an existing workspace platform's subdir and/or virtual packages                                   |
| [`move`](move/)     | Reorder a workspace platform, changing its selection priority                                          |
| [`list`](list/)     | List every workspace platform with full detail, preceded by the auto-detected host as a separate entry |
| [`remove`](remove/) | Remove platform(s) from the workspace file and updates the lock file                                   |

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
