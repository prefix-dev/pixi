# `pixi upload`

## About

Upload conda packages to various channels

The `pixi upload` command supports uploading conda packages to various server types:

| Server Type   | Description                                                         |
| ------------- | ------------------------------------------------------------------- |
| `prefix`      | Upload to [prefix.dev](https://prefix.dev) or self-hosted instances |
| `anaconda`    | Upload to [Anaconda.org](https://anaconda.org)                      |
| `quetz`       | Upload to a [Quetz](https://github.com/mamba-org/quetz) server      |
| `artifactory` | Upload to [JFrog Artifactory](https://jfrog.com/artifactory/)       |
| `s3`          | Upload to S3-compatible object storage                              |

## Usage

```text
pixi upload [OPTIONS] [PACKAGE_FILES]... <COMMAND>
```

## Subcommands

| Command                       | Description                                                                                          |
| ----------------------------- | ---------------------------------------------------------------------------------------------------- |
| [`quetz`](quetz/)             | Upload to a Quetz server. Authentication is used from the keychain / auth-file                       |
| [`artifactory`](artifactory/) | Options for uploading to a Artifactory channel. Authentication is used from the keychain / auth-file |
| [`prefix`](prefix/)           | Options for uploading to a prefix.dev server. Authentication is used from the keychain / auth-file   |
| [`anaconda`](anaconda/)       | Options for uploading to a Anaconda.org server                                                       |
| [`s3`](s3/)                   | Options for uploading to S3                                                                          |

## Arguments

- [`<PACKAGE_FILES>`](#arg-%3CPACKAGE_FILES%3E) : The package file to upload

  ```
  May be provided more than once.
  ```

## Options

- [`--allow-insecure-host <ALLOW_INSECURE_HOST>`](#arg---allow-insecure-host) : List of hosts for which SSL certificate verification should be skipped

  ```
  May be provided more than once.
  ```

## Description

Upload conda packages to various channels

Supported server types: prefix, anaconda, quetz, artifactory, s3, conda-forge

Use `pixi auth login` to authenticate with the server.

## Examples

### Uploading to prefix.dev

```shell
# Upload a package to a channel on prefix.dev
pixi upload prefix --channel my-channel my_package-1.0.0-h123abc_0.conda

# Upload with an explicit API key
pixi upload prefix --channel my-channel --api-key $PREFIX_API_KEY my_package.conda

# Skip upload if package already exists
pixi upload prefix --channel my-channel --skip-existing my_package.conda
```

### Uploading to Anaconda.org

```shell
# Upload to your personal channel
pixi upload anaconda --owner my-username my_package.conda

# Upload to a specific label/channel
pixi upload anaconda --owner my-username --channel dev my_package.conda

# Force replace existing package
pixi upload anaconda --owner my-username --force my_package.conda
```

### Uploading to S3

```shell
# Upload to an S3 bucket (using AWS credentials from environment)
pixi upload s3 --channel s3://my-bucket/my-channel my_package.conda

# Upload with explicit credentials
pixi upload s3 \
    --channel s3://my-bucket/my-channel \
    --region us-east-1 \
    --access-key-id $AWS_ACCESS_KEY_ID \
    --secret-access-key $AWS_SECRET_ACCESS_KEY \
    my_package.conda

# Upload to S3-compatible storage (MinIO, Cloudflare R2, etc.)
pixi upload s3 \
    --channel s3://my-bucket/my-channel \
    --endpoint-url https://minio.example.com \
    --region us-east-1 \
    --addressing-style path \
    my_package.conda

# Force replace existing package
pixi upload s3 --channel s3://my-bucket/my-channel --force my_package.conda
```

### Uploading to Quetz

```shell
# Upload to a Quetz server
pixi upload quetz \
    --url https://my-quetz-server.com \
    --channel my-channel \
    my_package.conda

# Upload with explicit API key
pixi upload quetz \
    --url https://my-quetz-server.com \
    --channel my-channel \
    --api-key $QUETZ_API_KEY \
    my_package.conda
```

### Uploading to Artifactory

```shell
# Upload to Artifactory
pixi upload artifactory \
    --url https://my-artifactory.com \
    --channel conda-local \
    my_package.conda

# Upload with explicit token
pixi upload artifactory \
    --url https://my-artifactory.com \
    --channel conda-local \
    --token $ARTIFACTORY_TOKEN \
    my_package.conda
```

### Uploading Multiple Packages

All server types support uploading multiple packages at once:

```shell
pixi upload prefix --channel my-channel package1.conda package2.conda package3.conda
```

## Authentication

For most server types, authentication can be provided in multiple ways:

1. **Keychain / auth-file**: Use `pixi auth login` to store credentials

   ```shell
   pixi auth login https://prefix.dev --token $MY_TOKEN
   pixi upload prefix --channel my-channel my_package.conda
   ```

1. **Environment variables**: Each server type supports specific environment variables

   - `PREFIX_API_KEY` for prefix.dev
   - `ANACONDA_API_KEY` for Anaconda.org
   - `QUETZ_API_KEY` for Quetz
   - `ARTIFACTORY_TOKEN` for Artifactory
   - `S3_ACCESS_KEY_ID`, `S3_SECRET_ACCESS_KEY` for S3

1. **Command-line arguments**: Pass credentials directly via `--api-key`, `--token`, etc.

## S3 Re-indexing

When uploading packages to S3, the `repodata.json` file needs to be updated manually since S3 is just storage, not a package server. Use `rattler-index` to re-index your S3 bucket after uploading:

```shell
pixi exec rattler-index s3 s3://my-bucket/my-channel \
    --endpoint-url https://my-s3-host \
    --region us-east-1
```

See the [S3 deployment documentation](../../../../deployment/s3/) for more details.
