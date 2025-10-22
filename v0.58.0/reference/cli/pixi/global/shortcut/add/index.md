# `[pixi](../../../) [global](../../) [shortcut](../) add`

## About

Add a shortcut from an environment to your machine.

## Usage

```text
pixi global shortcut add [OPTIONS] --environment <ENVIRONMENT> [PACKAGE]...

```

## Arguments

- [`<PACKAGE>`](#arg-%3CPACKAGE%3E) The package name to add the shortcuts from

  May be provided more than once.

## Options

- [`--environment (-e) <ENVIRONMENT>`](#arg---environment) The environment from which the shortcut should be added

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
