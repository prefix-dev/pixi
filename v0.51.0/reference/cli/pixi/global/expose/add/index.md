# `[pixi](../../../) [global](../../) [expose](../) add`

## About

Add exposed binaries from an environment to your global environment

## Usage

```text
pixi global expose add [OPTIONS] --environment <ENVIRONMENT> [MAPPING]...

```

## Arguments

- [`<MAPPING>`](#arg-%3CMAPPING%3E) Add mapping which describe which executables are exposed. The syntax is `exposed_name=executable_name`, so for example `python3.10=python`. Alternatively, you can input only an executable_name and `executable_name=executable_name` is assumed

  May be provided more than once.

## Options

- [`--environment (-e) <ENVIRONMENT>`](#arg---environment) The environment to which the binaries should be exposed

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

Add exposed binaries from an environment to your global environment

Example:

- `pixi global expose add python310=python3.10 python3=python3 --environment myenv`
- `pixi global add --environment my_env pytest pytest-cov --expose pytest=pytest`

## Examples

```shell
pixi global expose add python --environment my-env
pixi global expose add py310=python3.10 --environment python

```
