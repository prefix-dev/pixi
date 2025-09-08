# `[pixi](../../) [clean](../) cache`

## About

Clean the cache of your system which are touched by pixi

## Usage

```text
pixi clean cache [OPTIONS]

```

## Options

- [`--pypi`](#arg---pypi) Clean only the pypi related cache
- [`--conda`](#arg---conda) Clean only the conda related cache
- [`--mapping`](#arg---mapping) Clean only the mapping cache
- [`--exec`](#arg---exec) Clean only `exec` cache
- [`--repodata`](#arg---repodata) Clean only the repodata cache
- [`--build-backends`](#arg---build-backends) Clean only the build backends environments cache
- [`--build`](#arg---build) Clean only the build related cache
- [`--yes (-y)`](#arg---yes) Answer yes to all questions

## Description

Clean the cache of your system which are touched by pixi.

Specify the cache type to clean with the flags.

## Examples

```shell
pixi clean cache # clean all pixi caches
pixi clean cache --pypi # clean only the pypi cache
pixi clean cache --conda # clean only the conda cache
pixi clean cache --mapping # clean only the mapping cache
pixi clean cache --exec # clean only the `exec` cache
pixi clean cache --repodata # clean only the `repodata` cache
pixi clean cache --yes # skip the confirmation prompt

```
