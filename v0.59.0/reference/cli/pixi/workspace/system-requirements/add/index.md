# `[pixi](../../../) [workspace](../../) [system-requirements](../) add`

## About

Adds an environment to the manifest file

## Usage

```text
pixi workspace system-requirements add [OPTIONS] <REQUIREMENT> <VERSION>

```

## Arguments

- [`<REQUIREMENT>`](#arg-%3CREQUIREMENT%3E) The name of the system requirement to add

  **required**: `true`

  **options**: `linux`, `cuda`, `macos`, `glibc`, `other-libc`

- [`<VERSION>`](#arg-%3CVERSION%3E) The version of the requirement

  **required**: `true`

## Options

- [`--family <FAMILY>`](#arg---family) The Libc family, this can only be specified for requirement `other-libc`
- [`--feature (-f) <FEATURE>`](#arg---feature) The name of the feature to modify
