# `[pixi](../../) [global](../) add`

## About

Adds dependencies to an environment

## Usage

```text
pixi global add [OPTIONS] --environment <ENVIRONMENT> [PACKAGE]...

```

## Arguments

- [`<PACKAGE>`](#arg-%3CPACKAGE%3E) The dependency as names, conda MatchSpecs

  May be provided more than once.

## Options

- [`--path <PATH>`](#arg---path) The path to the local directory

- [`--environment (-e) <ENVIRONMENT>`](#arg---environment) Specifies the environment that the dependencies need to be added to

  **required**: `true`

- [`--expose <EXPOSE>`](#arg---expose) Add one or more mapping which describe which executables are exposed. The syntax is `exposed_name=executable_name`, so for example `python3.10=python`. Alternatively, you can input only an executable_name and `executable_name=executable_name` is assumed

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

## Git Options

- [`--git <GIT>`](#arg---git) The git url, e.g. `https://github.com/user/repo.git`
- [`--branch <BRANCH>`](#arg---branch) The git branch
- [`--tag <TAG>`](#arg---tag) The git tag
- [`--rev <REV>`](#arg---rev) The git revision
- [`--subdir <SUBDIR>`](#arg---subdir) The subdirectory within the git repository

## Description

Adds dependencies to an environment

Example:

- `pixi global add --environment python numpy`
- `pixi global add --environment my_env pytest pytest-cov --expose pytest=pytest`

## Examples

```shell
pixi global add python=3.9.* --environment my-env
pixi global add python=3.9.* --expose py39=python3.9 --environment my-env
pixi global add numpy matplotlib --environment my-env
pixi global add numpy matplotlib --expose np=python3.9 --environment my-env

```
