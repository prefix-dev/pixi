# `[pixi](../../../) [global](../../) [expose](../) remove`

## About

Remove exposed binaries from the global environment

## Usage

```text
pixi global expose remove [OPTIONS] [EXPOSED_NAME]...

```

## Arguments

- [`<EXPOSED_NAME>`](#arg-%3CEXPOSED_NAME%3E) The exposed names that should be removed Can be specified multiple times

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

## Description

Remove exposed binaries from the global environment

`pixi global expose remove python310 python3 --environment myenv` will remove the exposed names `python310` and `python3` from the environment `myenv`

## Examples

```shell
pixi global expose remove python
pixi global expose remove py310 python3

```
