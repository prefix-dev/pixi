# [pixi](../) tree

Show a tree of workspace dependencies

## Usage

```text
pixi tree [OPTIONS] [REGEX]
```

## Arguments

- [`<REGEX>`](#arg-%3CREGEX%3E) : List only packages matching a regular expression

## Options

- [`--platform (-p) <PLATFORM>`](#arg---platform) : The platform to list packages for. Defaults to the current platform
- [`--environment (-e) <ENVIRONMENT>`](#arg---environment) : The environment to list packages for. Defaults to the default environment
- [`--invert (-i)`](#arg---invert) : Invert tree and show what depends on given package in the regex argument

## Update Options

- [`--frozen`](#arg---frozen) : Install the environment as defined in the lock file, doesn't update lock file if it isn't up-to-date with the manifest file

  ```
  **env**: `PIXI_FROZEN`
  ```

- [`--locked`](#arg---locked) : Check if lock file is up-to-date before installing the environment, aborts when lock file isn't up-to-date with the manifest file

  ```
  **env**: `PIXI_LOCKED`
  ```

- [`--no-install`](#arg---no-install) : Don't modify the environment, only modify the lock file

  ```
  **env**: `PIXI_NO_INSTALL`
  ```

## Global Options

- [`--manifest-path (-m) <MANIFEST_PATH>`](#arg---manifest-path) : The path to `pixi.toml`, `pyproject.toml`, or the workspace directory
- [`--workspace (-w) <WORKSPACE>`](#arg---workspace) : Name of the workspace

## Description

Show a tree of workspace dependencies

Dependency names highlighted in green are directly specified in the manifest. Yellow version numbers are conda packages, PyPI version numbers are blue.

## Examples

```shell
pixi tree
pixi tree pre-commit
pixi tree -i yaml
pixi tree --environment docs
pixi tree --platform win-64
```

Output will look like this, where direct packages in the [manifest file](../../../pixi_manifest/) will be green. Once a package has been displayed once, the tree won't continue to recurse through its dependencies (compare the first time `python` appears, vs the rest), and it will instead be marked with a star `(*)`.

Version numbers are colored by the package type, yellow for Conda packages and blue for PyPI.

```shell
➜ pixi tree
├── pre-commit v3.3.3
│   ├── cfgv v3.3.1
│   │   └── python v3.12.2
│   │       ├── bzip2 v1.0.8
│   │       ├── libexpat v2.6.2
│   │       ├── libffi v3.4.2
│   │       ├── libsqlite v3.45.2
│   │       │   └── libzlib v1.2.13
│   │       ├── libzlib v1.2.13 (*)
│   │       ├── ncurses v6.4.20240210
│   │       ├── openssl v3.2.1
│   │       ├── readline v8.2
│   │       │   └── ncurses v6.4.20240210 (*)
│   │       ├── tk v8.6.13
│   │       │   └── libzlib v1.2.13 (*)
│   │       └── xz v5.2.6
│   ├── identify v2.5.35
│   │   └── python v3.12.2 (*)
...
└── tbump v6.9.0
...
    └── tomlkit v0.12.4
        └── python v3.12.2 (*)
```

A regex pattern can be specified to filter the tree to just those that show a specific direct, or transitive dependency:

```shell
➜ pixi tree pre-commit
└── pre-commit v3.3.3
    ├── virtualenv v20.25.1
    │   ├── filelock v3.13.1
    │   │   └── python v3.12.2
    │   │       ├── libexpat v2.6.2
    │   │       ├── readline v8.2
    │   │       │   └── ncurses v6.4.20240210
    │   │       ├── libsqlite v3.45.2
    │   │       │   └── libzlib v1.2.13
    │   │       ├── bzip2 v1.0.8
    │   │       ├── libzlib v1.2.13 (*)
    │   │       ├── libffi v3.4.2
    │   │       ├── tk v8.6.13
    │   │       │   └── libzlib v1.2.13 (*)
    │   │       ├── xz v5.2.6
    │   │       ├── ncurses v6.4.20240210 (*)
    │   │       └── openssl v3.2.1
    │   ├── platformdirs v4.2.0
    │   │   └── python v3.12.2 (*)
    │   ├── distlib v0.3.8
    │   │   └── python v3.12.2 (*)
    │   └── python v3.12.2 (*)
    ├── pyyaml v6.0.1
...
```

Additionally, the tree can be inverted, and it can show which packages depend on a regex pattern. The packages specified in the manifest will also be highlighted (in this case `cffconvert` and `pre-commit` would be).

```shell
➜ pixi tree -i yaml

ruamel.yaml v0.18.6
├── pykwalify v1.8.0
│   └── cffconvert v2.0.0
└── cffconvert v2.0.0

pyyaml v6.0.1
└── pre-commit v3.3.3

ruamel.yaml.clib v0.2.8
└── ruamel.yaml v0.18.6
    ├── pykwalify v1.8.0
    │   └── cffconvert v2.0.0
    └── cffconvert v2.0.0

yaml v0.2.5
└── pyyaml v6.0.1
    └── pre-commit v3.3.3
```

Warning

Use `-v` to show which `pypi` packages are not yet parsed correctly. The `extras` and `markers` parsing is still under development.
