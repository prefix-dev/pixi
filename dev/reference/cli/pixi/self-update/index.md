# [pixi](../) self-update

Update pixi to the latest version or a specific version

## Usage

```text
pixi self-update [OPTIONS]
```

## Options

- [`--offline=<OFFLINE>`](#arg---offline) : Run without network access. Updating always requires the network, so this makes `pixi self-update` fail fast instead of attempting to connect

  ```
  **env**: `PIXI_OFFLINE`
    
  **options**: `y`, `yes`, `t`, `true`, `on`, `1`, `n`, `no`, `f`, `false`, `off`, `0`
  ```

- [`--version <VERSION>`](#arg---version) : The desired version (to downgrade or upgrade to)

- [`--dry-run`](#arg---dry-run) : Only show release notes, do not modify the binary

- [`--force`](#arg---force) : Force download the desired version when not exactly same with the current. If no desired version, always replace with the latest version

  ```
  **default**: `false`
  ```

- [`--no-release-note`](#arg---no-release-note) : Skip printing the release notes

  ```
  **default**: `false`
  ```

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

## Examples

```shell
pixi self-update
pixi self-update --version 0.46.0
```
