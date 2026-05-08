# [pixi](../) publish

Build a conda package and publish it to a channel.

`pixi publish` **builds** a conda package from your workspace and **uploads** it to a channel.

- With a target URL: builds and uploads to the specified destination.
- Without a target URL: builds and copies the package to the current working directory.

The target URL determines the upload backend:

| URL pattern                      | Backend                                             |
| -------------------------------- | --------------------------------------------------- |
| `https://prefix.dev/<channel>`   | [prefix.dev](https://prefix.dev)                    |
| `https://anaconda.org/<owner>`   | [Anaconda.org](https://anaconda.org)                |
| `s3://bucket-name/channel`       | S3-compatible storage                               |
| `channel:///path/to/channel`     | Local filesystem channel                            |
| `file:///path/to/dir`            | Copy to local directory                             |
| `quetz://server/<channel>`       | [Quetz](https://github.com/mamba-org/quetz)         |
| `artifactory://server/<channel>` | [JFrog Artifactory](https://jfrog.com/artifactory/) |

## Usage

```text
pixi publish [OPTIONS]
```

## Options

- [`--target-platform (-t) <TARGET_PLATFORM>`](#arg---target-platform) : The target platform to build for (defaults to the current platform)

  ```
  **default**: `current_platform`
  ```

- [`--build-platform <BUILD_PLATFORM>`](#arg---build-platform) : The build platform to use for building (defaults to the current platform)

  ```
  **default**: `current_platform`
  ```

- [`--build-string-prefix <BUILD_STRING_PREFIX>`](#arg---build-string-prefix) : An optional prefix prepended to the auto-generated build string

- [`--build-number <BUILD_NUMBER>`](#arg---build-number) : An optional override for the package's build number

- [`--build-dir (-b) <BUILD_DIR>`](#arg---build-dir) : The directory to use for incremental builds artifacts

- [`--clean (-c)`](#arg---clean) : Whether to clean the build directory before building

- [`--path <PATH>`](#arg---path) : The path to a directory containing a package manifest, or to a specific manifest file

- [`--target-channel <TARGET_CHANNEL>`](#arg---target-channel) : The target channel URL to publish packages to

- [`--target-dir <TARGET_DIR>`](#arg---target-dir) : The target local directory to copy packages into (no channel indexing)

- [`--force`](#arg---force) : Force overwrite existing packages

- [`--skip-existing <SKIP_EXISTING>`](#arg---skip-existing) : Skip uploading packages that already exist on the target channel. This is enabled by default. Use `--no-skip-existing` to disable

  ```
  **default**: `true`
    
  **options**: `true`, `false`
  ```

- [`--generate-attestation`](#arg---generate-attestation) : Generate sigstore attestation (prefix.dev only)

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

- [`--tls-root-certs <TLS_ROOT_CERTS>`](#arg---tls-root-certs) : Which TLS root certificates to use: 'webpki' (bundled Mozilla roots), 'native' (system store), or 'all' (both)

  ```
  **env**: `PIXI_TLS_ROOT_CERTS`
  ```

- [`--use-environment-activation-cache`](#arg---use-environment-activation-cache) : Use environment activation cache (experimental)

## Description

Build a conda package and publish it to a channel.

This is a convenience command that combines `pixi build` and `pixi upload`.

Supported target URLs (--target-channel / --to):

- prefix.dev: `https://prefix.dev/<channel-name>`
- anaconda.org: `https://anaconda.org/<owner>/<label>`
- S3: `s3://bucket-name`
- Local channel (with indexing): `channel:///path/to/channel`
- Local path (copy only): `file:///path/to/output`
- Quetz: `quetz://server/<channel>`
- Artifactory: `artifactory://server/<channel>`

## Examples

### Build and copy to current directory

Omit the target URL to build and copy the resulting package to the current working directory.

```shell
# Build the package and copy it to the current directory
pixi publish
```

### Publishing to prefix.dev

The most common use case is publishing packages to a channel on [prefix.dev](https://prefix.dev).

```shell
# Build and publish to your prefix.dev channel
pixi publish https://prefix.dev/my-channel

# Build for a specific target platform and publish
pixi publish https://prefix.dev/my-channel --target-platform linux-64

# Publish with sigstore attestation for supply chain security
pixi publish https://prefix.dev/my-channel --generate-attestation

# Force overwrite existing packages
pixi publish https://prefix.dev/my-channel --force
```

For authentication, either log in first with `pixi auth login`, or set the `PREFIX_API_KEY` environment variable:

```shell
# Option 1: Log in (credentials are stored in the keychain)
pixi auth login --token $MY_TOKEN https://prefix.dev
pixi publish https://prefix.dev/my-channel

# Option 2: Use trusted publishing in CI (no credentials needed)
pixi publish https://prefix.dev/my-channel
```

See the [prefix.dev deployment guide](../../../../deployment/prefix/) for more details on setting up channels and trusted publishing.

### Publishing to Anaconda.org

```shell
# Build and publish to your Anaconda.org account
pixi publish https://anaconda.org/my-username

# Publish to a specific label/channel
pixi publish https://anaconda.org/my-username/dev
```

### Publishing to S3

When publishing to S3, the channel is automatically initialized (if new) and indexed after the upload.

```shell
# Build and publish to an S3 bucket (using AWS credentials from environment)
pixi publish s3://my-bucket/my-channel
```

S3 credentials are resolved from the standard AWS credential chain (environment variables, shared credentials file, instance profiles, etc.).

### Publishing to a local filesystem channel

Local channels are useful for development and testing. The channel directory is automatically created and indexed.

```shell
# Build and publish to a local channel
pixi publish channel:///path/to/my-channel

# Force overwrite if the package already exists
pixi publish channel:///path/to/my-channel --force
```

### Copying to a local path

Use `file://` or a bare path to copy the built `.conda` artifact directly to a directory, without creating a channel structure.

```shell
# Copy the built package to a directory (file:// URL)
pixi publish file:///path/to/output/dir

# Bare paths also work -- relative or absolute
pixi publish /tmp/my-packages
pixi publish ../my-packages
```

If the directory looks like a conda channel (contains `repodata.json`), pixi warns that you may have meant to use `channel://` instead.

### Publishing from a specific manifest

```shell
# Publish a package from a specific recipe
pixi publish https://prefix.dev/my-channel --path ./my-recipe/recipe.yaml

# Publish from a different workspace
pixi publish https://prefix.dev/my-channel --path ./my-project/
```

### Clean rebuild and publish

```shell
# Clean the build directory before building and publishing
pixi publish https://prefix.dev/my-channel --clean
```

## Authentication

`pixi publish` uses the same authentication system as other pixi commands. Credentials can be configured in three ways:

1. **Keychain / auth-file**: Use `pixi auth login` to store credentials

   ```shell
   pixi auth login --token $MY_TOKEN https://prefix.dev
   ```

1. **Trusted publishing (CI)**: On GitHub Actions, GitLab CI, or Google Cloud, prefix.dev supports OIDC-based trusted publishing — no stored secrets required. See the [prefix.dev trusted publishing guide](../../../../deployment/prefix/#trusted-publishing).

1. **Environment variables**:

   - `PREFIX_API_KEY` for prefix.dev
   - `ANACONDA_API_KEY` for Anaconda.org
   - `QUETZ_API_KEY` for Quetz
   - `ARTIFACTORY_TOKEN` for Artifactory
   - Standard AWS environment variables for S3
