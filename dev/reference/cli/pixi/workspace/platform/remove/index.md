# `[pixi](../../../) [workspace](../../) [platform](../) remove`

## About

Remove platform(s) from the workspace file and updates the lockfile

## Usage

```text
pixi workspace platform remove [OPTIONS] <PLATFORM>...

```

## Arguments

- [`<PLATFORM>`](#arg-%3CPLATFORM%3E) The platform name to remove

  May be provided more than once.

  **required**: `true`

## Options

- [`--no-install`](#arg---no-install) Don't update the environment, only remove the platform(s) from the lock-file
- [`--feature (-f) <FEATURE>`](#arg---feature) The name of the feature to remove the platform from
