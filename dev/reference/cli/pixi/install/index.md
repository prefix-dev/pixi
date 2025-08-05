# `[pixi](../) install`

## About

Install an environment, both updating the lockfile and installing the environment

## Usage

```text
pixi install [OPTIONS]

```

## Options

- [`--environment (-e) <ENVIRONMENT>`](#arg---environment) The environment to install

  May be provided more than once.

- [`--all (-a)`](#arg---all) Install all environments

- [`--skip <SKIP>`](#arg---skip) Skip installation of specific packages present in the lockfile. Requires --frozen. This can be useful for instance in a Dockerfile to skip local source dependencies when installing dependencies

  May be provided more than once.

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

- [`--frozen`](#arg---frozen) Install the environment as defined in the lockfile, doesn't update lockfile if it isn't up-to-date with the manifest file

  **env**: `PIXI_FROZEN`

- [`--locked`](#arg---locked) Check if lockfile is up-to-date before installing the environment, aborts when lockfile isn't up-to-date with the manifest file

  **env**: `PIXI_LOCKED`

## Global Options

- [`--manifest-path <MANIFEST_PATH>`](#arg---manifest-path) The path to `pixi.toml`, `pyproject.toml`, or the workspace directory

## Description

Install an environment, both updating the lockfile and installing the environment.

This command installs an environment, if the lockfile is not up-to-date it will be updated.

`pixi install` only installs one environment at a time, if you have multiple environments you can select the right one with the `--environment` flag. If you don't provide an environment, the `default` environment will be installed.

If you want to install all environments, you can use the `--all` flag.

Running `pixi install` is not required before running other commands like `pixi run` or `pixi shell`. These commands will automatically install the environment if it is not already installed.

You can use `pixi reinstall` to reinstall all environments, one environment or just some packages of an environment.

## Examples

```shell
pixi install  # (1)!
pixi install --manifest-path ~/myworkspace/pixi.toml # (2)!
pixi install --frozen # (3)!
pixi install --locked # (4)!
pixi install --environment lint # (5)!
pixi install -e lint # (5)!

```

1. This will install the default environment.
1. This will install the default environment from the manifest file at the given path.
1. This will install the environment from the lockfile without updating the lockfile.
1. This will install the environment from the lockfile without updating the lockfile and ensuring the environment is locked correctly.
1. This will install the `lint` environment.
