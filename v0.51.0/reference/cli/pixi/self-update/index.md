# `[pixi](../) self-update`

## About

Update pixi to the latest version or a specific version

## Usage

```text
pixi self-update [OPTIONS]

```

## Options

- [`--version <VERSION>`](#arg---version) The desired version (to downgrade or upgrade to)

- [`--dry-run`](#arg---dry-run) Only show release notes, do not modify the binary

- [`--force`](#arg---force) Force download the desired version when not exactly same with the current. If no desired version, always replace with the latest version

  **default**: `false`

- [`--no-release-note`](#arg---no-release-note) Skip printing the release notes

  **default**: `false`

## Examples

```shell
pixi self-update
pixi self-update --version 0.46.0

```
