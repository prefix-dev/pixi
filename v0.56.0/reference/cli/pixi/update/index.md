# `[pixi](../) update`

## About

The `update` command checks if there are newer versions of the dependencies and updates the `pixi.lock` file and environments accordingly

## Usage

```text
pixi update [OPTIONS] [PACKAGES]...

```

## Arguments

- [`<PACKAGES>`](#arg-%3CPACKAGES%3E) The packages to update, space separated. If no packages are provided, all packages will be updated

  May be provided more than once.

## Options

- [`--no-install`](#arg---no-install) Don't install the (solve) environments needed for pypi-dependencies solving

- [`--dry-run (-n)`](#arg---dry-run) Don't actually write the lockfile or update any environment

- [`--environment (-e) <ENVIRONMENTS>`](#arg---environment) The environments to update. If none is specified, all environments are updated

  May be provided more than once.

- [`--platform (-p) <PLATFORMS>`](#arg---platform) The platforms to update. If none is specified, all platforms are updated

  May be provided more than once.

- [`--json`](#arg---json) Output the changes in JSON format

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

## Global Options

- [`--manifest-path <MANIFEST_PATH>`](#arg---manifest-path) The path to `pixi.toml`, `pyproject.toml`, or the workspace directory

## Description

The `update` command checks if there are newer versions of the dependencies and updates the `pixi.lock` file and environments accordingly.

It will only update the lock file if the dependencies in the manifest file are still compatible with the new versions.

## Examples

```shell
pixi update numpy # (1)!
pixi update numpy pandas # (2)!
pixi update --manifest-path ~/myworkspace/pixi.toml numpy # (3)!
pixi update --environment lint python # (4)!
pixi update -e lint -e schema -e docs pre-commit # (5)!
pixi update --platform osx-arm64 mlx # (6)!
pixi update -p linux-64 -p osx-64 numpy  # (7)!
pixi update --dry-run numpy # (8)!
pixi update --no-install boto3 # (9)!

```

1. This will update the `numpy` package to the latest version that fits the requirement.
1. This will update the `numpy` and `pandas` packages to the latest version that fits the requirement.
1. This will update the `numpy` package to the latest version in the manifest file at the given path.
1. This will update the `python` package in the `lint` environment.
1. This will update the `pre-commit` package in the `lint`, `schema`, and `docs` environments.
1. This will update the `mlx` package in the `osx-arm64` platform.
1. This will update the `numpy` package in the `linux-64` and `osx-64` platforms.
1. This will show the packages that would be updated without actually updating them in the lockfile
1. This will update the `boto3` package in the manifest and lockfile, without installing it in an environment.
