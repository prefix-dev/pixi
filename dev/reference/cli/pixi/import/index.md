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

- [`--platform (-p) <PLATFORM>`](#arg---platform) : The platforms for the imported environment. Accepts a workspace platform name; a bare conda subdir (e.g. `linux-64`) is also accepted. Names that aren't yet declared get auto-added as subdir platforms

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

- [`--no-symbolic-links`](#arg---no-symbolic-links) : Disallow symbolic links during package installation

  ```
  **env**: `PIXI_NO_SYMBOLIC_LINKS`
  ```

- [`--no-hard-links`](#arg---no-hard-links) : Disallow hard links during package installation

  ```
  **env**: `PIXI_NO_HARD_LINKS`
  ```

- [`--no-ref-links`](#arg---no-ref-links) : Disallow ref links (copy-on-write) during package installation

  ```
  **env**: `PIXI_NO_REF_LINKS`
  ```

- [`--tls-no-verify`](#arg---tls-no-verify) : Do not verify the TLS certificate of the server

- [`--tls-root-certs <TLS_ROOT_CERTS>`](#arg---tls-root-certs) : Which TLS root certificates to use: 'webpki' (bundled Mozilla roots) or 'system' (system store)

  ```
  **env**: `PIXI_TLS_ROOT_CERTS`
  ```

- [`--use-environment-activation-cache`](#arg---use-environment-activation-cache) : Use environment activation cache (experimental)

- [`--no-config`](#arg---no-config) : Don't read system or user-level configuration files. Project-local `<project>/.pixi/config.toml` is still loaded

  ```
  **env**: `PIXI_NO_CONFIG`
    
  **default**: `false`
  ```

- [`--config-file <PATH>`](#arg---config-file) : Load configuration from this file instead of searching system and user-level paths. Project-local `<project>/.pixi/config.toml` is still merged on top

  ```
  **env**: `PIXI_CONFIG_FILE`
  ```

## Global Options

- [`--manifest-path (-m) <MANIFEST_PATH>`](#arg---manifest-path) : The path to `pixi.toml`, `pyproject.toml`, or the workspace directory
- [`--workspace (-w) <WORKSPACE>`](#arg---workspace) : Name of the workspace

## Description

Imports a file into an environment in an existing workspace.

If `--format` isn't provided, `import` will try each format in turn
