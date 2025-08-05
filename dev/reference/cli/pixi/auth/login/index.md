# `[pixi](../../) [auth](../) login`

## About

Store authentication information for a given host

## Usage

```text
pixi auth login [OPTIONS] <HOST>

```

## Arguments

- [`<HOST>`](#arg-%3CHOST%3E) The host to authenticate with (e.g. repo.prefix.dev)

  **required**: `true`

## Options

- [`--token <TOKEN>`](#arg---token) The token to use (for authentication with prefix.dev)
- [`--username <USERNAME>`](#arg---username) The username to use (for basic HTTP authentication)
- [`--password <PASSWORD>`](#arg---password) The password to use (for basic HTTP authentication)
- [`--conda-token <CONDA_TOKEN>`](#arg---conda-token) The token to use on anaconda.org / quetz authentication
- [`--s3-access-key-id <S3_ACCESS_KEY_ID>`](#arg---s3-access-key-id) The S3 access key ID
- [`--s3-secret-access-key <S3_SECRET_ACCESS_KEY>`](#arg---s3-secret-access-key) The S3 secret access key
- [`--s3-session-token <S3_SESSION_TOKEN>`](#arg---s3-session-token) The S3 session token

## Examples

```shell
pixi auth login repo.prefix.dev --token pfx_JQEV-m_2bdz-D8NSyRSaAndHANx0qHjq7f2iD
pixi auth login anaconda.org --conda-token ABCDEFGHIJKLMNOP
pixi auth login https://myquetz.server --username john --password xxxxxx
pixi auth login s3://my-bucket --s3-access-key-id $AWS_ACCESS_KEY_ID --s3-access-key-id $AWS_SECRET_KEY_ID

```
