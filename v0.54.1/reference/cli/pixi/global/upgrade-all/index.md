# `[pixi](../../) [global](../) upgrade-all`

## About

Upgrade all globally installed packages This command has been removed, please use `pixi global update` instead

## Usage

```text
pixi global upgrade-all [OPTIONS]

```

## Options

- [`--channel (-c) <CHANNEL>`](#arg---channel) The channels to consider as a name or a url. Multiple channels can be specified by using this field multiple times

  May be provided more than once.

- [`--platform <PLATFORM>`](#arg---platform) The platform to install the package for

  **default**: `current_platform`

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
