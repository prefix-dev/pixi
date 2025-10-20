# `[pixi](../../../) [workspace](../../) [platform](../) add`

## About

Adds a platform(s) to the workspace file and updates the lockfile

## Usage

```text
pixi workspace platform add [OPTIONS] <PLATFORM>...

```

## Arguments

- [`<PLATFORM>`](#arg-%3CPLATFORM%3E) The platform name(s) to add

  May be provided more than once.

  **required**: `true`

## Options

- [`--no-install`](#arg---no-install) Don't update the environment, only add changed packages to the lock-file
- [`--feature (-f) <FEATURE>`](#arg---feature) The name of the feature to add the platform to
