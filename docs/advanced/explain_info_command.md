---
part: pixi/advanced
title: Info command
description: Learn what the info command reports
---

`pixi info` prints out useful information to debug a situation or to get an overview of your machine/project.
This information can also be retrieved in `json` format using the `--json` flag, which can be useful for programmatically reading it.

```title="Running pixi info in the pixi repo"
➜ pixi info
pixi 0.0.7

Platform            : linux-64
Virtual packages    : __unix=0=0
                    : __linux=6.4.4=0
                    : __glibc=2.36=0
                    : __cuda=12.2=0
                    : __archspec=1=x86_64
Cache dir           : /home/user/.cache/rattler/cache
Auth storage        : /home/user/.rattler

Project
------------

Manifest file       : /home/user/development/pixi/pixi.toml
Dependency count    : 6
Last updated        : 22-07-2023 13:31:30
Target platforms    : linux-64
                    : win-64
                    : osx-64
                    : osx-arm64
```

#### Options

- `--extended`: Gives more information that would otherwise be too slow for command.
  This shows the sizes of the directories.
- `--json`: Get a machine-readable version of the information as output.

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

Pixi caches all previously downloaded packages in a cache folder.
This cache folder is shared between all pixi projects and globally installed tools.
Normally the locations would be:

| Platform | Value                                               |
|----------|-----------------------------------------------------|
| Linux    | `$XDG_CACHE_HOME/rattler` or `$HOME`/.cache/rattler |
| macOS    | `$HOME`/Library/Caches/rattler                      |
| Windows  | `{FOLDERID_LocalAppData}/rattler`                   |

When your system is filling up you can easily remove this folder.
It will re-download everything it needs the next time you install a project.

### Auth storage

Check the [authentication documentation](authentication.md)

### Cache size

[requires `--extended`]

The size of the previously mentioned "Cache dir" in Mebibytes.

## Project info

Everything below `Project` is info about the project you're currently in.
This info is only available if your path has a manifest file (`pixi.toml`).

### Manifest file

The path to the manifest file that describes the project.
For now, this can only be `pixi.toml`.

### Dependency count

The amount of dependencies defined in the manifest file.

### Last updated

The last time the lockfile was updated, either manually or by pixi itself.

### Target platforms

The platforms the project has defined.

### Environment size

[requires `--extended`]

The size of the `.pixi` folder in Mebibytes.
