# `[pixi](../) reinstall`

## About

Re-install an environment, both updating the lockfile and re-installing the environment

## Usage

```text
pixi reinstall [OPTIONS] [PACKAGE]...

```

## Arguments

- [`<PACKAGE>`](#arg-%3CPACKAGE%3E) Specifies the package that should be reinstalled. If no package is given, the whole environment will be reinstalled

  May be provided more than once.

## Options

- [`--environment (-e) <ENVIRONMENT>`](#arg---environment) The environment to install

  May be provided more than once.

- [`--all (-a)`](#arg---all) Install all environments

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

## Description

Re-install an environment, both updating the lockfile and re-installing the environment.

This command reinstalls an environment, if the lockfile is not up-to-date it will be updated. If packages are specified, only those packages will be reinstalled. Otherwise the whole environment will be reinstalled.

`pixi reinstall` only re-installs one environment at a time, if you have multiple environments you can select the right one with the `--environment` flag. If you don't provide an environment, the `default` environment will be re-installed.

If you want to re-install all environments, you can use the `--all` flag.
