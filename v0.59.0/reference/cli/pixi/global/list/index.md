# `[pixi](../../) [global](../) list`

## About

Lists global environments with their dependencies and exposed commands. Can also display all packages within a specific global environment when using the --environment flag.

## Usage

```text
pixi global list [OPTIONS] [REGEX]

```

## Arguments

- [`<REGEX>`](#arg-%3CREGEX%3E) List only packages matching a regular expression. Without regex syntax it acts like a `contains` filter

## Options

- [`--environment (-e) <ENVIRONMENT>`](#arg---environment) Allows listing all the packages installed in a specific environment, with an output similar to `pixi list`

- [`--sort-by <SORT_BY>`](#arg---sort-by) Sorting strategy for the package table of an environment

  **default**: `name`

  **options**: `size`, `name`

## Config Options

- [`--auth-file <AUTH_FILE>`](#arg---auth-file) Path to the file containing the authentication token

- [`--concurrent-downloads <CONCURRENT_DOWNLOADS>`](#arg---concurrent-downloads) Max concurrent network requests, default is `50`

- [`--concurrent-solves <CONCURRENT_SOLVES>`](#arg---concurrent-solves) Max concurrent solves, default is the number of CPUs

- [`--pinning-strategy <PINNING_STRATEGY>`](#arg---pinning-strategy) Set pinning strategy

  **options**: `semver`, `minor`, `major`, `latest-up`, `exact-version`, `no-pin`

- [`--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>`](#arg---pypi-keyring-provider) Specifies whether to use the keyring to look up credentials for PyPI

  **options**: `disabled`, `subprocess`

- [`--run-post-link-scripts`](#arg---run-post-link-scripts) Run post-link scripts (insecure)

- [`--tls-no-verify`](#arg---tls-no-verify) Do not verify the TLS certificate of the server

- [`--use-environment-activation-cache`](#arg---use-environment-activation-cache) Use environment activation cache (experimental)

## Description

Lists global environments with their dependencies and exposed commands. Can also display all packages within a specific global environment when using the --environment flag.

All environments:

- Yellow: the binaries that are exposed.
- Green: the packages that are explicit dependencies of the environment.
- Blue: the version of the installed package.
- Cyan: the name of the environment.

Per environment:

- Green: packages that are explicitly installed.

## Examples

We'll only show the dependencies and exposed binaries of the environment if they differ from the environment name. Here is an example of a few installed packages:

```text
pixi global list

```

Results in:

```text
Global environments at /home/user/.pixi:
├── gh: 2.57.0
├── pixi-pack: 0.1.8
├── python: 3.11.0
│   └─ exposes: 2to3, 2to3-3.11, idle3, idle3.11, pydoc, pydoc3, pydoc3.11, python, python3, python3-config, python3.1, python3.11, python3.11-config
├── rattler-build: 0.22.0
├── ripgrep: 14.1.0
│   └─ exposes: rg
├── vim: 9.1.0611
│   └─ exposes: ex, rview, rvim, view, vim, vimdiff, vimtutor, xxd
└── zoxide: 0.9.6

```

Here is an example of list of a single environment:

```text
pixi g list -e pixi-pack

```

Results in:

```text
The 'pixi-pack' environment has 8 packages:
Package          Version    Build        Size
_libgcc_mutex    0.1        conda_forge  2.5 KiB
_openmp_mutex    4.5        2_gnu        23.1 KiB
ca-certificates  2024.8.30  hbcca054_0   155.3 KiB
libgcc           14.1.0     h77fa898_1   826.5 KiB
libgcc-ng        14.1.0     h69a702a_1   50.9 KiB
libgomp          14.1.0     h77fa898_1   449.4 KiB
openssl          3.3.2      hb9d3cd8_0   2.8 MiB
pixi-pack        0.1.8      hc762bcd_0   4.3 MiB
Package          Version    Build        Size
Exposes:
pixi-pack
Channels:
conda-forge
Platform: linux-64

```
