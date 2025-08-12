# `[pixi](../../../) [workspace](../../) [export](../) conda-explicit-spec`

## About

Export workspace environment to a conda explicit specification file

## Usage

```text
pixi workspace export conda-explicit-spec [OPTIONS] <OUTPUT_DIR>

```

## Arguments

- [`<OUTPUT_DIR>`](#arg-%3COUTPUT_DIR%3E) Output directory for rendered explicit environment spec files

  **required**: `true`

## Options

- [`--environment (-e) <ENVIRONMENT>`](#arg---environment) The environments to render. Can be repeated for multiple environments

  May be provided more than once.

- [`--platform (-p) <PLATFORM>`](#arg---platform) The platform to render. Can be repeated for multiple platforms. Defaults to all platforms available for selected environments

  May be provided more than once.

- [`--ignore-pypi-errors`](#arg---ignore-pypi-errors) PyPI dependencies are not supported in the conda explicit spec file

  **default**: `false`

- [`--ignore-source-errors`](#arg---ignore-source-errors) Source dependencies are not supported in the conda explicit spec file

  **default**: `false`

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

- [`--no-lockfile-update`](#arg---no-lockfile-update) Don't update lockfile, implies the no-install as well

- [`--frozen`](#arg---frozen) Install the environment as defined in the lockfile, doesn't update lockfile if it isn't up-to-date with the manifest file

  **env**: `PIXI_FROZEN`

- [`--locked`](#arg---locked) Check if lockfile is up-to-date before installing the environment, aborts when lockfile isn't up-to-date with the manifest file

  **env**: `PIXI_LOCKED`

## Global Options

- [`--manifest-path <MANIFEST_PATH>`](#arg---manifest-path) The path to `pixi.toml`, `pyproject.toml`, or the workspace directory
