# [pixi](../../../) [global](../../) [shortcut](../) add

Add a shortcut from an environment to your machine.

## Usage

```text
pixi global shortcut add [OPTIONS] --environment <ENVIRONMENT> [PACKAGE]...
```

## Arguments

- [`<PACKAGE>`](#arg-%3CPACKAGE%3E) : The package name to add the shortcuts from

  ```
  May be provided more than once.
  ```

## Options

- [`--environment (-e) <ENVIRONMENT>`](#arg---environment) : The environment from which the shortcut should be added

  ```
  **required**: `true`
  ```

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
