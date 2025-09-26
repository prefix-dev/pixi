# `[pixi](../) upgrade`

## About

Checks if there are newer versions of the dependencies and upgrades them in the lockfile and manifest file

Note

The `pixi upgrade` command will update only `version`s, except when you specify the exact package name (`pixi upgrade numpy`).

Then it will remove all fields, apart from:

- `build` field containing a wildcard `*`
- `channel`
- `file_name`
- `url`
- `subdir`.

Note

In v0.55.0 and earlier releases, by default only the `default` feature was upgraded. Pass `--feature=default` if you want to emulate this behaviour on newer releases.

## Usage

```text
pixi upgrade [OPTIONS] [PACKAGES]...

```

## Arguments

- [`<PACKAGES>`](#arg-%3CPACKAGES%3E) The packages to upgrade

  May be provided more than once.

## Options

- [`--feature (-f) <FEATURE>`](#arg---feature) The feature to update

- [`--exclude <EXCLUDE>`](#arg---exclude) The packages which should be excluded

  May be provided more than once.

- [`--json`](#arg---json) Output the changes in JSON format

- [`--dry-run (-n)`](#arg---dry-run) Only show the changes that would be made, without actually updating the manifest, lock file, or environment

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

## Update Options

- [`--no-install`](#arg---no-install) Don't modify the environment, only modify the lock-file

- [`--frozen`](#arg---frozen) Install the environment as defined in the lockfile, doesn't update lockfile if it isn't up-to-date with the manifest file

  **env**: `PIXI_FROZEN`

- [`--locked`](#arg---locked) Check if lockfile is up-to-date before installing the environment, aborts when lockfile isn't up-to-date with the manifest file

  **env**: `PIXI_LOCKED`

## Global Options

- [`--manifest-path <MANIFEST_PATH>`](#arg---manifest-path) The path to `pixi.toml`, `pyproject.toml`, or the workspace directory

## Description

Checks if there are newer versions of the dependencies and upgrades them in the lockfile and manifest file.

`pixi upgrade` loosens the requirements for the given packages, updates the lock file and the adapts the manifest accordingly. By default, all features are upgraded.

## Examples

```shell
pixi upgrade # (1)!
pixi upgrade numpy # (2)!
pixi upgrade numpy pandas # (3)!
pixi upgrade --manifest-path ~/myworkspace/pixi.toml numpy # (4)!
pixi upgrade --feature lint python # (5)!
pixi upgrade --json # (6)!
pixi upgrade --dry-run # (7)!

```

1. This will upgrade all packages to the latest version.
1. This will upgrade the `numpy` package to the latest version.
1. This will upgrade the `numpy` and `pandas` packages to the latest version.
1. This will upgrade the `numpy` package to the latest version in the manifest file at the given path.
1. This will upgrade the `python` package in the `lint` feature.
1. This will upgrade all packages and output the result in JSON format.
1. This will show the packages that would be upgraded without actually upgrading them in the lockfile or manifest.
