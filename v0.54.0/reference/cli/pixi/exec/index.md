# `[pixi](../) exec`

## About

Run a command and install it in a temporary environment

## Usage

```text
pixi exec [OPTIONS] [COMMAND]...

```

## Arguments

- [`<COMMAND>`](#arg-%3CCOMMAND%3E) The executable to run, followed by any arguments

  May be provided more than once.

## Options

- [`--spec (-s) <SPEC>`](#arg---spec) Matchspecs of package to install. If this is not provided, the package is guessed from the command

  May be provided more than once.

- [`--with (-w) <WITH>`](#arg---with) Matchspecs of package to install, while also guessing a package from the command

  May be provided more than once.

- [`--channel (-c) <CHANNEL>`](#arg---channel) The channels to consider as a name or a url. Multiple channels can be specified by using this field multiple times

  May be provided more than once.

- [`--platform (-p) <PLATFORM>`](#arg---platform) The platform to create the environment for

  **default**: `current_platform`

- [`--force-reinstall`](#arg---force-reinstall) If specified a new environment is always created even if one already exists

- [`--list <LIST>`](#arg---list) Before executing the command, list packages in the environment Specify `--list=some_regex` to filter the shown packages

- [`--no-modify-ps1`](#arg---no-modify-ps1) Disable modification of the PS1 prompt to indicate the temporary environment

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

Run a command and install it in a temporary environment.

Remove the temporary environments with `pixi clean cache --exec`.

## Examples

```shell
pixi exec python
# Run ipython and include the py-rattler and numpy packages in the environment
pixi exec --with py-rattler --with numpy ipython
# Specify the specs of the environment
pixi exec --spec python=3.9 --spec numpy python
# Force reinstall to recreate the environment and get the latest package versions
pixi exec --force-reinstall --with py-rattler ipython

```
