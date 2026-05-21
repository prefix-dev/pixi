# [pixi](../../) [workspace](../) register

Commands to manage the registry of workspaces. Default command will add a new workspace

## Usage

```text
pixi workspace register [OPTIONS] [COMMAND]
```

## Subcommands

| Command             | Description                                  |
| ------------------- | -------------------------------------------- |
| [`list`](list/)     | List the registered workspaces               |
| [`remove`](remove/) | Remove a workspace from registry             |
| [`prune`](prune/)   | Prune disassociated workspaces from registry |

## Options

- [`--name (-n) <NAME>`](#arg---name) : Name of the workspace to register. Defaults to the name of the current workspace
- [`--path (-p) <PATH>`](#arg---path) : Path to register. Defaults to the path to the current workspace
- [`--force (-f)`](#arg---force) : Overwrite the workspace entry if the name of the workspace already exists in the registry

## Global Options

- [`--manifest-path (-m) <MANIFEST_PATH>`](#arg---manifest-path) : The path to `pixi.toml`, `pyproject.toml`, or the workspace directory
- [`--workspace (-w) <WORKSPACE>`](#arg---workspace) : Name of the workspace
