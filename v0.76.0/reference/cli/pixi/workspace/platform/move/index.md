# [pixi](../../../) [workspace](../../) [platform](../) move

Reorder a workspace platform, changing its selection priority

## Usage

```text
pixi workspace platform move [OPTIONS] <--before <PLATFORM>|--after <PLATFORM>|--to-top|--to-bottom> <NAME>
```

## Arguments

- [`<NAME>`](#arg-%3CNAME%3E) : Name of the platform to move

  ```
  **required**: `true`
  ```

## Options

- [`--before <PLATFORM>`](#arg---before) : Move it directly before this platform

- [`--after <PLATFORM>`](#arg---after) : Move it directly after this platform

- [`--to-top`](#arg---to-top) : Move it to the top of the list (highest selection priority)

- [`--to-bottom`](#arg---to-bottom) : Move it to the bottom of the list (lowest selection priority)

- [`--no-install`](#arg---no-install) : Don't update the environment, only refresh the lock-file

  ```
  **env**: `PIXI_NO_INSTALL`
  ```
