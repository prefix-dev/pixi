# [pixi](../) publish

Build a conda package and publish it to a channel.

`pixi publish` **builds** a conda package from your workspace and **uploads** it to a channel or copies it into a directory.

- With `--target-channel <URL>` (alias `--to`): builds and uploads to the specified channel.
- With `--target-dir <PATH>`: builds and copies the package(s) into the given directory (no channel indexing).

`--target-channel` and `--target-dir` are mutually exclusive. If neither is provided, the default is `--target-dir .` (copy to the current working directory).

The `--target-channel` value determines the upload backend:

| URL pattern                                               | Backend                                             |
| --------------------------------------------------------- | --------------------------------------------------- |
| `https://prefix.dev/<channel>`                            | [prefix.dev](https://prefix.dev)                    |
| `https://anaconda.org/<owner>`                            | [Anaconda.org](https://anaconda.org)                |
| `s3://bucket-name/channel`                                | S3-compatible storage                               |
| `quetz://server/<channel>`                                | [Quetz](https://github.com/mamba-org/quetz)         |
| `artifactory://server/<channel>`                          | [JFrog Artifactory](https://jfrog.com/artifactory/) |
| `file:///path/to/channel` or a bare path like `./example` | Local filesystem channel (indexed)                  |

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

- [`--target-channel <TARGET_CHANNEL>`](#arg---target-channel) : The target channel to publish packages to. Accepts a URL (prefix.dev, anaconda.org, s3://, quetz://, artifactory://) or a local filesystem path / `file://` URL for an indexed local channel

  ```
  **aliases**: to
  ```

- [`--target-dir <TARGET_DIR>`](#arg---target-dir) : The local filesystem path to copy the built package(s) into (no channel indexing)

- [`--force`](#arg---force) : Force overwrite existing packages

- [`--skip-existing <SKIP_EXISTING>`](#arg---skip-existing) : Skip uploading packages that already exist at the target. This is enabled by default. Use `--no-skip-existing` to disable

  ```
  **default**: `true`
    
  **options**: `true`, `false`
  ```

- [`--generate-attestation`](#arg---generate-attestation) : Generate sigstore attestation (prefix.dev only)

- [`--variant <KEY=VALUES>`](#arg---variant) : Override a build variant key with one or more values

  ```
  May be provided more than once.
  ```

- [`--variant-config (-m) <FILE>`](#arg---variant-config) : Path to an additional variant configuration YAML file

  ```
  May be provided more than once.
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

## Description

Build a conda package and publish it to a channel.

Builds the package from your workspace and either uploads it to a channel (`--target-channel`) or copies the artifact into a local directory (`--target-dir`).

Supported destinations for `--target-channel` (alias `--to`):

- prefix.dev: `https://prefix.dev/<channel-name>`
- anaconda.org: `https://anaconda.org/<owner>/<label>`
- S3: `s3://bucket-name`
- Quetz: `quetz://server/<channel>`
- Artifactory: `artifactory://server/<channel>`
- Local filesystem channel (with indexing): `file:///path/to/channel` or a bare path

Use `--target-dir <PATH>` instead to copy the built package(s) into a directory without creating a channel structure.

## Examples

### Publishing to prefix.dev

The most common use case is publishing packages to a channel on [prefix.dev](https://prefix.dev).

```shell
# Build and publish to your prefix.dev channel
pixi publish --target-channel https://prefix.dev/my-channel

# Build for a specific target platform and publish
pixi publish --target-channel https://prefix.dev/my-channel --target-platform linux-64

# Publish with sigstore attestation for supply chain security
pixi publish --target-channel https://prefix.dev/my-channel --generate-attestation

# Force overwrite existing packages
pixi publish --target-channel https://prefix.dev/my-channel --force
```

For authentication, either log in first with `pixi auth login`, or set the `PREFIX_API_KEY` environment variable:

```shell
# Option 1: Log in (credentials are stored in the keychain)
pixi auth login --token $MY_TOKEN https://prefix.dev
pixi publish --target-channel https://prefix.dev/my-channel

# Option 2: Use trusted publishing in CI (no credentials needed)
pixi publish --target-channel https://prefix.dev/my-channel
```

See the [prefix.dev deployment guide](../../../../deployment/prefix/) for more details on setting up channels and trusted publishing.

### Publishing to Anaconda.org

```shell
# Build and publish to your Anaconda.org account
pixi publish --target-channel https://anaconda.org/my-username

# Publish to a specific label/channel
pixi publish --target-channel https://anaconda.org/my-username/dev
```

### Publishing to S3

When publishing to S3, the channel is automatically initialized (if new) and indexed after the upload.

```shell
# Build and publish to an S3 bucket (using AWS credentials from environment)
pixi publish --target-channel s3://my-bucket/my-channel
```

S3 credentials are resolved from the standard AWS credential chain (environment variables, shared credentials file, instance profiles, etc.).

### Publishing to a local filesystem channel

Local channels are useful for development and testing. The channel directory is automatically created and indexed. Pass a `file://` URL or a bare path to `--target-channel`.

```shell
# Build and publish to a local channel (bare path)
pixi publish --target-channel /path/to/my-channel

# Equivalent, using a file:// URL
pixi publish --target-channel file:///path/to/my-channel

# Force overwrite if the package already exists
pixi publish --target-channel /path/to/my-channel --force
```

### Copying to a local directory

Use `--target-dir` to copy the built `.conda` artifact directly to a directory, without creating a channel structure.

```shell
# Copy the built package(s) to a directory
pixi publish --target-dir /path/to/output/dir

# Relative paths work too
pixi publish --target-dir ../my-packages
```

### Publishing from a specific manifest

```shell
# Publish a package from a specific recipe
pixi publish --target-channel https://prefix.dev/my-channel --path ./my-recipe/recipe.yaml

# Publish from a different workspace
pixi publish --target-channel https://prefix.dev/my-channel --path ./my-project/
```

### Clean rebuild and publish

```shell
# Clean the build directory before building and publishing
pixi publish --target-channel https://prefix.dev/my-channel --clean
```

### Overriding build variants

By default `pixi publish` builds every entry in the [`[workspace.build-variants]`](../../../pixi_manifest/#build-variants-optional) matrix. Use `--variant KEY=VALUE` to override individual variant keys at the command line — for iterating locally on a single variant, parallelizing CI jobs across the matrix, or anything else where you don't want the full cross-product.

```shell
# Build just one entry of the matrix
pixi publish --variant python=3.12

# Constrain a variant to a subset of values (cross-product with the
# remaining workspace variants)
pixi publish --variant python=3.12 --variant cuda-version=12.8,13.0
```

CLI overrides replace the matching key from the workspace `build-variants`; workspace keys that aren't named on the command line keep their full value list.

For shared variant configuration, prefer listing the YAML files in [`[workspace.build-variants-files]`](../../../pixi_manifest/#build-variants-files-optional) in `pixi.toml` so every contributor and CI run picks them up automatically. The `--variant-config` / `-m` flag is meant for one-off overrides — for example, an extra file you don't want to commit. CLI-supplied files are appended after the workspace files, so values from `-m` override any matching key from the workspace files (`rattler-build --variant-config` semantics).

```shell
# Use an extra variant file for this build only
pixi publish -m variants.yaml

# Combine with a key override
pixi publish -m variants.yaml --variant python=3.12
```

## Authentication

`pixi publish` uses the same authentication system as other pixi commands. Credentials can be configured in three ways:

1. **Keychain / auth-file**: Use `pixi auth login` to store credentials

   ```shell
   pixi auth login --token $MY_TOKEN https://prefix.dev
   ```

1. **Trusted publishing (CI)**: On GitHub Actions, GitLab CI, or Google Cloud, prefix.dev supports OIDC-based trusted publishing -- no stored secrets required. See the [prefix.dev trusted publishing guide](../../../../deployment/prefix/#trusted-publishing).

1. **Environment variables**:

   - `PREFIX_API_KEY` for prefix.dev
   - `ANACONDA_API_KEY` for Anaconda.org
   - `QUETZ_API_KEY` for Quetz
   - `ARTIFACTORY_TOKEN` for Artifactory
   - Standard AWS environment variables for S3
