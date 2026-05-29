# [pixi](../../) [workspace](../) requires-pixi

Commands to manage the pixi minimum version requirement

## Usage

```text
pixi workspace requires-pixi [OPTIONS] <COMMAND>
```

## Subcommands

| Command             | Description                                 |
| ------------------- | ------------------------------------------- |
| [`get`](get/)       | Get the pixi minimum version requirement    |
| [`set`](set/)       | Set the pixi minimum version requirement    |
| [`unset`](unset/)   | Remove the pixi minimum version requirement |
| [`verify`](verify/) | Verify the pixi minimum version requirement |

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
