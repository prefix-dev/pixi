# [pixi](../) lock

Solve environment and update the lock file without installing the environments

## Usage

```text
pixi lock [OPTIONS]
```

## Options

- [`--json`](#arg---json) : Output the changes in JSON format
- [`--check`](#arg---check) : Check if any changes have been made to the lock file. If yes, exit with a non-zero code
- [`--dry-run`](#arg---dry-run) : Compute the lock file without writing to disk. Implies --no-install

## Update Options

- [`--no-install`](#arg---no-install) : Don't modify the environment, only modify the lock-file

## Global Options

- [`--manifest-path (-m) <MANIFEST_PATH>`](#arg---manifest-path) : The path to `pixi.toml`, `pyproject.toml`, or the workspace directory

## Examples

```shell
pixi lock
pixi lock --manifest-path ~/myworkspace/pixi.toml
pixi lock --json
pixi lock --check
```
