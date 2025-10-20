# `[pixi](../../) [global](../) uninstall`

## About

Uninstalls environments from the global environment.

## Usage

```text
pixi global uninstall [OPTIONS] <ENVIRONMENT>...

```

## Arguments

- [`<ENVIRONMENT>`](#arg-%3CENVIRONMENT%3E) Specifies the environments that are to be removed

  May be provided more than once.

  **required**: `true`

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

Uninstalls environments from the global environment.

Example: `pixi global uninstall pixi-pack rattler-build`

## Examples

```shell
pixi global uninstall my-env
pixi global uninstall pixi-pack rattler-build

```
