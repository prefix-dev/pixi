---
part: pixi/advanced
title: Info command
description: Learn what the info command reports
---

`pixi info` prints out useful information to debug a situation or to get an overview of your machine/project.
This information can also be retrieved in `json` format using the `--json` flag, which can be useful for programmatically reading it.

```title="Running pixi info in the pixi repo"
➜ pixi info
      Pixi version: 0.13.0
          Platform: linux-64
  Virtual packages: __unix=0=0
                  : __linux=6.5.12=0
                  : __glibc=2.36=0
                  : __cuda=12.3=0
                  : __archspec=1=x86_64
         Cache dir: /home/user/.cache/rattler/cache
      Auth storage: /home/user/.rattler/credentials.json

Project
------------
           Version: 0.13.0
     Manifest file: /home/user/development/pixi/pixi.toml
      Last updated: 25-01-2024 10:29:08

Environments
------------
default
          Features: default
          Channels: conda-forge
  Dependency count: 10
      Dependencies: pre-commit, rust, openssl, pkg-config, git, mkdocs, mkdocs-material, pillow, cairosvg, compilers
  Target platforms: linux-64, osx-arm64, win-64, osx-64
             Tasks: docs, test-all, test, build, lint, install, build-docs
```

## Global info

The first part of the info output is information that is always available and tells you what pixi can read on your machine.

### Platform

This defines the platform you're currently on according to pixi.
If this is incorrect, please file an issue on the [pixi repo](https://github.com/prefix-dev/pixi).

### Virtual packages

The virtual packages that pixi can find on your machine.

In the Conda ecosystem, you can depend on virtual packages.
These packages aren't real dependencies that are going to be installed, but rather are being used in the solve step to find if a package can be installed on the machine.
A simple example: When a package depends on Cuda drivers being present on the host machine it can do that by depending on the `__cuda` virtual package.
In that case, if pixi cannot find the `__cuda` virtual package on your machine the installation will fail.

### Cache dir

The directory where pixi stores its cache.
Checkout the [cache documentation](../features/environment.md#caching-packages) for more information.

### Auth storage

Check the [authentication documentation](authentication.md)

### Cache size

[requires `--extended`]

The size of the previously mentioned "Cache dir" in Mebibytes.

## Project info

Everything below `Project` is info about the project you're currently in.
This info is only available if your path has a [manifest file](../reference/pixi_manifest.md).

### Manifest file

The path to the [manifest file](../reference/pixi_manifest.md) that describes the project.

### Last updated

The last time the lock file was updated, either manually or by pixi itself.

## Environment info

The environment info defined per environment. If you don't have any environments defined, this will only show the `default` environment.

### Features

This lists which features are enabled in the environment.
For the default this is only `default`

### Channels

The list of channels used in this environment.

### Dependency count

The amount of dependencies defined that are defined for this environment (not the amount of installed dependencies).

### Dependencies

The list of dependencies defined for this environment.

### Target platforms

The platforms the project has defined.
