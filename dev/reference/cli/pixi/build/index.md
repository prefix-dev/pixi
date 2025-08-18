# `[pixi](../) build`

## About

Workspace configuration

## Usage

```text
pixi build [OPTIONS]

```

## Options

- [`--target-platform (-t) <TARGET_PLATFORM>`](#arg---target-platform) The target platform to build for (defaults to the current platform)

  **default**: `current_platform`

- [`--build-platform <BUILD_PLATFORM>`](#arg---build-platform) The build platform to use for building (defaults to the current platform)

  **default**: `current_platform`

- [`--output-dir (-o) <OUTPUT_DIR>`](#arg---output-dir) The output directory to place the built artifacts

  **default**: `.`

- [`--build-dir (-b) <BUILD_DIR>`](#arg---build-dir) The directory to use for incremental builds artifacts

- [`--clean (-c)`](#arg---clean) Whether to clean the build directory before building

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

## Global Options

- [`--manifest-path <MANIFEST_PATH>`](#arg---manifest-path) The path to `pixi.toml`, `pyproject.toml`, or the workspace directory
