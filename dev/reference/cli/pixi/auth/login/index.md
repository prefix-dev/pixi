# [pixi](../../) [auth](../) login

Store authentication information for a given host

## Usage

```text
pixi auth login [OPTIONS] <HOST>
```

## Arguments

- [`<HOST>`](#arg-%3CHOST%3E) : The host to authenticate with (e.g. prefix.dev)

  ```
  **required**: `true`
  ```

## OAuth/OIDC Authentication

- [`--oauth`](#arg---oauth) : Use OAuth/OIDC authentication

- [`--oauth-issuer-url <OAUTH_ISSUER_URL>`](#arg---oauth-issuer-url) : OIDC issuer URL (defaults to <https://%7Bhost>})

- [`--oauth-client-id <OAUTH_CLIENT_ID>`](#arg---oauth-client-id) : OAuth client ID (defaults to "rattler")

- [`--oauth-client-secret <OAUTH_CLIENT_SECRET>`](#arg---oauth-client-secret) : OAuth client secret (for confidential clients)

- [`--oauth-flow <OAUTH_FLOW>`](#arg---oauth-flow) : OAuth flow: auto (default), auth-code, device-code

  ```
  **options**: `auto`, `auth-code`, `device-code`
  ```

- [`--oauth-scope <OAUTH_SCOPES>`](#arg---oauth-scope) : Additional OAuth scopes to request (repeatable)

  ```
  May be provided more than once.
  ```

## S3 Authentication

- [`--s3-access-key-id <S3_ACCESS_KEY_ID>`](#arg---s3-access-key-id) : The S3 access key ID
- [`--s3-secret-access-key <S3_SECRET_ACCESS_KEY>`](#arg---s3-secret-access-key) : The S3 secret access key
- [`--s3-session-token <S3_SESSION_TOKEN>`](#arg---s3-session-token) : The S3 session token

## Token / Basic Authentication

- [`--token <TOKEN>`](#arg---token) : The token to use (for authentication with prefix.dev)
- [`--username <USERNAME>`](#arg---username) : The username to use (for basic HTTP authentication)
- [`--password <PASSWORD>`](#arg---password) : The password to use (for basic HTTP authentication)
- [`--conda-token <CONDA_TOKEN>`](#arg---conda-token) : The token to use on anaconda.org / quetz authentication

## Examples

```shell
pixi auth login repo.prefix.dev --token pfx_JQEV-m_2bdz-D8NSyRSaAndHANx0qHjq7f2iD
pixi auth login anaconda.org --conda-token ABCDEFGHIJKLMNOP
pixi auth login https://myquetz.server --username john --password xxxxxx
pixi auth login s3://my-bucket --s3-access-key-id $AWS_ACCESS_KEY_ID --s3-secret-access-key $AWS_SECRET_ACCESS_KEY
```
