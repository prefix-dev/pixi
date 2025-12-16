# [pixi](../../) [workspace](../) channel

Commands to manage workspace channels

## Usage

```text
pixi workspace channel [OPTIONS] <COMMAND>
```

## Subcommands

| Command             | Description                                                  |
| ------------------- | ------------------------------------------------------------ |
| [`add`](add/)       | Adds a channel to the manifest and updates the lockfile      |
| [`list`](list/)     | List the channels in the manifest                            |
| [`remove`](remove/) | Remove channel(s) from the manifest and updates the lockfile |

## Global Options

- [`--manifest-path (-m) <MANIFEST_PATH>`](#arg---manifest-path) : The path to `pixi.toml`, `pyproject.toml`, or the workspace directory
