# `[pixi](../../) [global](../) remove`

## About

Removes dependencies from an environment

## Usage

```text
pixi global remove [OPTIONS] <PACKAGE>...

```

## Arguments

- [`<PACKAGE>`](#arg-%3CPACKAGE%3E) Specifies the package that should be removed

  May be provided more than once.

  **required**: `true`

## Options

- [`--environment (-e) <ENVIRONMENT>`](#arg---environment) Specifies the environment that the dependencies need to be removed from

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

Removes dependencies from an environment

Use `pixi global uninstall` to remove the whole environment

Example: `pixi global remove --environment python numpy`

## Examples

```shell
pixi global remove -e my-env package1 package2

```
