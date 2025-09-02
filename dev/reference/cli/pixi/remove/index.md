# `[pixi](../) remove`

## About

Removes dependencies from the workspace

If the project manifest is a `pyproject.toml`, removing a pypi dependency with the `--pypi` flag will remove it from either

- the native pyproject `project.dependencies` array or the native `project.optional-dependencies` table (if a feature is specified)
- pixi `pypi-dependencies` tables of the default or a named feature (if a feature is specified)

## Usage

```text
pixi remove [OPTIONS] <SPEC>...

```

## Arguments

- [`<SPEC>`](#arg-%3CSPEC%3E) The dependency as names, conda MatchSpecs or PyPi requirements

  May be provided more than once.

  **required**: `true`

## Options

- [`--pypi`](#arg---pypi) The specified dependencies are pypi dependencies. Conflicts with `host` and `build`

- [`--platform (-p) <PLATFORM>`](#arg---platform) The platform for which the dependency should be modified

  May be provided more than once.

- [`--feature (-f) <FEATURE>`](#arg---feature) The feature for which the dependency should be modified

  **default**: `default`

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

- [`--git (-g) <GIT>`](#arg---git) The git url to use when adding a git dependency
- [`--branch <BRANCH>`](#arg---branch) The git branch
- [`--tag <TAG>`](#arg---tag) The git tag
- [`--rev <REV>`](#arg---rev) The git revision
- [`--subdir (-s) <SUBDIR>`](#arg---subdir) The subdirectory of the git repository to use

## Update Options

- [`--no-install`](#arg---no-install) Don't modify the environment, only modify the lock-file

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

Removes dependencies from the workspace.

If the workspace manifest is a `pyproject.toml`, removing a pypi dependency with the `--pypi` flag will remove it from either

- the native pyproject `project.dependencies` array or, if a feature is specified, the native `project.optional-dependencies` table

- pixi `pypi-dependencies` tables of the default feature or, if a feature is specified, a named feature

## Examples

```shell
pixi remove numpy
pixi remove numpy pandas pytorch
pixi remove --manifest-path ~/myworkspace/pixi.toml numpy
pixi remove --host python
pixi remove --build cmake
pixi remove --pypi requests
pixi remove --platform osx-64 --build clang
pixi remove --feature featurex clang
pixi remove --feature featurex --platform osx-64 clang
pixi remove --feature featurex --platform osx-64 --build clang
pixi remove --no-install numpy

```
