# `[pixi](../) import`

## About

Imports a file into an environment in an existing workspace.

## Usage

```text
pixi import [OPTIONS] <FILE>

```

## Arguments

- [`<FILE>`](#arg-%3CFILE%3E) File to import into the workspace

  **required**: `true`

## Options

- [`--format <FORMAT>`](#arg---format) Which format to interpret the file as

  **options**: `conda-env`

- [`--platform (-p) <PLATFORM>`](#arg---platform) The platforms for the imported environment

  May be provided more than once.

- [`--environment (-e) <ENVIRONMENT>`](#arg---environment) A name for the created environment

- [`--feature (-f) <FEATURE>`](#arg---feature) A name for the created feature

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

- [`--revalidate`](#arg---revalidate) Run the complete environment validation. This will reinstall a broken environment

- [`--no-lockfile-update`](#arg---no-lockfile-update) Don't update lockfile, implies the no-install as well

- [`--frozen`](#arg---frozen) Install the environment as defined in the lockfile, doesn't update lockfile if it isn't up-to-date with the manifest file

  **env**: `PIXI_FROZEN`

- [`--locked`](#arg---locked) Check if lockfile is up-to-date before installing the environment, aborts when lockfile isn't up-to-date with the manifest file

  **env**: `PIXI_LOCKED`

## Global Options

- [`--manifest-path <MANIFEST_PATH>`](#arg---manifest-path) The path to `pixi.toml`, `pyproject.toml`, or the workspace directory

## Description

Imports a file into an environment in an existing workspace.

If `--format` isn't provided, `import` will try to guess the format based on the file extension.
