# [pixi](../) publish

Build the conda packages of a workspace and publish them to a channel.

`pixi publish` **builds** the conda packages of your workspace and **uploads** them to a channel or copies them into a directory.

By default every workspace package that opts into publishing with `publish = true` in its `[package]` section is published:

```toml
[package]
name = "my-package"
publish = true
```

Packages are discovered by walking the workspace directory tree. The walk respects ignore files such as `.gitignore` and skips subdirectories that contain their own workspace. Packages that do not set `publish = true` are left out of the publish set; if no package opts in, the package at the current directory is published instead, as if `--path .` had been passed.

Packages are built and uploaded in dependency order, so a package is never uploaded before the workspace packages it depends on. The published set must be closed: every source dependency (build, host, or run) of a published package must itself opt into publishing. Depending on a package outside the publish set, on a git or url source, or on a path outside the workspace root fails the publish - intentionally, so a publish can never leave the target channel referencing packages that were not uploaded. Replace external source dependencies with binary dependencies to publish.

Use `--dry-run` to resolve, validate, and print the publish set without building or uploading anything.

Use `--path <dir>` to build and publish a single package instead - whether or not it sets `publish = true`. A `--path` publish is a batch of one, so the package must be self-contained: none of its run dependencies may be a source dependency. Build and host source dependencies are only consumed while building and are fine.

- With `--target-channel <URL>` (alias `--to`): builds and uploads to the specified channel.
- With `--target-dir <PATH>`: builds and copies the package(s) into the given directory (no channel indexing).

`--target-channel` and `--target-dir` are mutually exclusive. If neither is provided, the default is `--target-dir .` (copy to the current working directory).

The `--target-channel` value determines the upload backend:

| URL pattern                                               | Backend                                             |
| --------------------------------------------------------- | --------------------------------------------------- |
| `https://prefix.dev/<channel>`                            | [prefix.dev](https://prefix.dev)                    |
| `https://anaconda.org/<owner>`                            | [Anaconda.org](https://anaconda.org)                |
| `cloudsmith://<owner>/<repository>/`                      | [Cloudsmith](https://cloudsmith.com)                |
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

- [`--target-channel <TARGET_CHANNEL>`](#arg---target-channel) : The target channel to publish packages to. Accepts a URL (prefix.dev, anaconda.org, cloudsmith://, s3://, quetz://, artifactory://) or a local filesystem path / `file://` URL for an indexed local channel

  ```
  **aliases**: to
  ```

- [`--target-dir <TARGET_DIR>`](#arg---target-dir) : The local filesystem path to copy the built package(s) into (no channel indexing)

- [`--force`](#arg---force) : Force overwrite existing packages

- [`--no-skip-existing`](#arg---no-skip-existing) : Do not skip packages that already exist at the target

- [`--dry-run`](#arg---dry-run) : Resolve and print the publish set without building or uploading

- [`--generate-attestation`](#arg---generate-attestation) : Generate sigstore attestation (prefix.dev only)

- [`--variant <KEY=VALUES>`](#arg---variant) : Override a build variant key with one or more values

  ```
  May be provided more than once.
  ```

- [`--variant-config (-m) <FILE>`](#arg---variant-config) : Path to an additional variant configuration YAML file

  ```
  May be provided more than once.
  ```

- [`--package-format <PACKAGE_FORMAT>`](#arg---package-format) : Archive format and optional compression level, e.g. `conda`, `tar-bz2`, `conda:max`, `conda:15`, `tar-bz2:9`. Numeric ranges match rattler-build: -7..=22 for `.conda`, 1..=9 for `.tar.bz2`

## Config Options

- [`--no-config`](#arg---no-config) : Don't read system or user-level configuration files. Project-local `<project>/.pixi/config.toml` is still loaded

  ```
  **env**: `PIXI_NO_CONFIG`
    
  **default**: `false`
  ```

- [`--config-file <PATH>`](#arg---config-file) : Load configuration from this file instead of searching system and user-level paths. Project-local `<project>/.pixi/config.toml` is still merged on top

  ```
  **env**: `PIXI_CONFIG_FILE`
  ```

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

- [`--offline=<OFFLINE>`](#arg---offline) : Run without network access, using only cached data. Commands fail if data is missing from the cache. Pass `--offline=false` to override an `offline` setting from the configuration

  ```
  **env**: `PIXI_OFFLINE`
    
  **options**: `y`, `yes`, `t`, `true`, `on`, `1`, `n`, `no`, `f`, `false`, `off`, `0`
  ```

- [`--tls-root-certs <TLS_ROOT_CERTS>`](#arg---tls-root-certs) : Which TLS root certificates to use: 'webpki' (bundled Mozilla roots) or 'system' (system store)

  ```
  **env**: `PIXI_TLS_ROOT_CERTS`
  ```

- [`--use-environment-activation-cache`](#arg---use-environment-activation-cache) : Use environment activation cache (experimental)

## Description

Build the conda packages of a workspace and publish them to a channel.

Builds every package in the workspace that opts into publishing with `publish = true` in its `[package]` section - in dependency order, so packages that depend on other workspace packages are built after them - and either uploads the artifacts to a channel (`--target-channel`) or copies them into a local directory (`--target-dir`). Every source dependency of a published package must itself opt into publishing; the publish fails otherwise. When no package opts in, the package at the current directory is published instead, as if `--path .` had been passed. Use `--path` to build and publish a single package.

Supported destinations for `--target-channel` (alias `--to`):

- prefix.dev: `https://prefix.dev/<channel-name>`
- anaconda.org: `https://anaconda.org/<owner>/<label>`
- Cloudsmith: `cloudsmith://<owner>/<repository>`
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

### Publishing to Cloudsmith

Pass the Cloudsmith channel URL as the target. Pixi derives the owner and repository from that URL and uses Cloudsmith's API for the upload.

```shell
# Build and publish to a Cloudsmith Conda repository
pixi publish --to cloudsmith://my-org/my-repository/
```

For authentication, set `CLOUDSMITH_API_KEY` or store credentials for the Cloudsmith API URL. Set `CLOUDSMITH_API_URL` only when targeting a custom Cloudsmith API endpoint.

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

### Publishing a single package

When `--path` is given, only the addressed package is built and published, whether or not it sets `publish = true`. None of its run dependencies may be a source dependency.

```shell
# Publish a single package of the workspace
pixi publish --target-channel https://prefix.dev/my-channel --path ./my-package/

# Publish a package from a specific recipe
pixi publish --target-channel https://prefix.dev/my-channel --path ./my-recipe/recipe.yaml
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
   - `CLOUDSMITH_API_KEY` for Cloudsmith
   - `QUETZ_API_KEY` for Quetz
   - `ARTIFACTORY_TOKEN` for Artifactory
   - Standard AWS environment variables for S3
