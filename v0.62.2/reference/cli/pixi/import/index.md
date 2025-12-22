# [pixi](../) import

Imports a file into an environment in an existing workspace.

## Usage

```text
pixi import [OPTIONS] <FILE>
```

## Arguments

- [`<FILE>`](#arg-%3CFILE%3E) : File to import into the workspace

  ```
  **required**: `true`
  ```

## Options

- [`--format <FORMAT>`](#arg---format) : Which format to interpret the file as

  ```
  **options**: `conda-env`, `pypi-txt`
  ```

- [`--platform (-p) <PLATFORM>`](#arg---platform) : The platforms for the imported environment

  ```
  May be provided more than once.
  ```

- [`--environment (-e) <ENVIRONMENT>`](#arg---environment) : A name for the created environment

- [`--feature (-f) <FEATURE>`](#arg---feature) : A name for the created feature

## Config Options

- [`--auth-file <AUTH_FILE>`](#arg---auth-file) : Path to the file containing the authentication token

- [`--concurrent-downloads <CONCURRENT_DOWNLOADS>`](#arg---concurrent-downloads) : Max concurrent network requests, default is `50`

- [`--concurrent-solves <CONCURRENT_SOLVES>`](#arg---concurrent-solves) : Max concurrent solves, default is the number of CPUs

- [`--pinning-strategy <PINNING_STRATEGY>`](#arg---pinning-strategy) : Set pinning strategy

  ```
  **options**: `semver`, `minor`, `major`, `latest-up`, `exact-version`, `no-pin`
  ```

- [`--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>`](#arg---pypi-keyring-provider) : Specifies whether to use the keyring to look up credentials for PyPI

  ```
  **options**: `disabled`, `subprocess`
  ```

- [`--run-post-link-scripts`](#arg---run-post-link-scripts) : Run post-link scripts (insecure)

- [`--tls-no-verify`](#arg---tls-no-verify) : Do not verify the TLS certificate of the server

- [`--tls-root-certs <TLS_ROOT_CERTS>`](#arg---tls-root-certs) : Which TLS root certificates to use: 'webpki' (bundled Mozilla roots), 'native' (system store), or 'all' (both)

  ```
  **env**: `PIXI_TLS_ROOT_CERTS`
  ```

- [`--use-environment-activation-cache`](#arg---use-environment-activation-cache) : Use environment activation cache (experimental)

## Global Options

- [`--manifest-path (-m) <MANIFEST_PATH>`](#arg---manifest-path) : The path to `pixi.toml`, `pyproject.toml`, or the workspace directory

## Description

Imports a file into an environment in an existing workspace.

If `--format` isn't provided, `import` will try each format in turn
