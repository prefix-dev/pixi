# [pixi](../../../) [workspace](../../) [channel](../) add

Adds a channel to the manifest and updates the lockfile

## Usage

```text
pixi workspace channel add [OPTIONS] <CHANNEL>...
```

## Arguments

- [`<CHANNEL>`](#arg-%3CCHANNEL%3E) : The channel name or URL

  ```
  May be provided more than once.
    
  **required**: `true`
  ```

## Options

- [`--priority <PRIORITY>`](#arg---priority) : Specify the channel priority
- [`--prepend`](#arg---prepend) : Add the channel(s) to the beginning of the channels list, making them the highest priority
- [`--feature (-f) <FEATURE>`](#arg---feature) : The name of the feature to modify

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

## Update Options

- [`--no-install`](#arg---no-install) : Don't modify the environment, only modify the lock-file

  ```
  **env**: `PIXI_NO_INSTALL`
  ```

- [`--frozen`](#arg---frozen) : Install the environment as defined in the lockfile, doesn't update lockfile if it isn't up-to-date with the manifest file

  ```
  **env**: `PIXI_FROZEN`
  ```

- [`--locked`](#arg---locked) : Check if lockfile is up-to-date before installing the environment, aborts when lockfile isn't up-to-date with the manifest file

  ```
  **env**: `PIXI_LOCKED`
  ```
