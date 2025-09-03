# `[pixi](../../../) [workspace](../../) [channel](../) add`

## About

Adds a channel to the manifest and updates the lockfile

## Usage

```text
pixi workspace channel add [OPTIONS] <CHANNEL>...

```

## Arguments

- [`<CHANNEL>`](#arg-%3CCHANNEL%3E) The channel name or URL

  May be provided more than once.

  **required**: `true`

## Options

- [`--priority <PRIORITY>`](#arg---priority) Specify the channel priority
- [`--prepend`](#arg---prepend) Add the channel(s) to the beginning of the channels list, making them the highest priority
- [`--feature (-f) <FEATURE>`](#arg---feature) The name of the feature to modify

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

- [`--frozen=<FROZEN>`](#arg---frozen) Install the environment as defined in the lockfile, doesn't update lockfile if it isn't up-to-date with the manifest file

  **env**: `PIXI_FROZEN`

  **default**: `false`

  **options**: `true`, `false`

- [`--locked=<LOCKED>`](#arg---locked) Check if lockfile is up-to-date before installing the environment, aborts when lockfile isn't up-to-date with the manifest file

  **env**: `PIXI_LOCKED`

  **default**: `false`

  **options**: `true`, `false`

## Global Options

- [`--manifest-path <MANIFEST_PATH>`](#arg---manifest-path) The path to `pixi.toml`, `pyproject.toml`, or the workspace directory
