You can authenticate Pixi with a server like prefix.dev, a private quetz instance, anaconda.org, or any S3-compatible bucket. Different servers use different authentication methods. This page covers how to authenticate against each, and where the authentication information is stored.

```shell
Usage: pixi auth login [OPTIONS] <HOST>

Arguments:
  <HOST>  The host to authenticate with (e.g. prefix.dev)

Options:
      --user-agent <USER_AGENT>  User-Agent header used for requests

Token / Basic Authentication:
      --token <TOKEN>              The token to use (for authentication with prefix.dev)
      --username <USERNAME>        The username to use (for basic HTTP authentication)
      --password <PASSWORD>        The password to use (for basic HTTP authentication)
      --conda-token <CONDA_TOKEN>  The token to use on anaconda.org / quetz authentication

S3 Authentication:
      --s3-access-key-id <S3_ACCESS_KEY_ID>          The S3 access key ID
      --s3-secret-access-key <S3_SECRET_ACCESS_KEY>  The S3 secret access key
      --s3-session-token <S3_SESSION_TOKEN>          The S3 session token

OAuth/OIDC Authentication:
      --oauth                                      Use OAuth/OIDC authentication
      --oauth-issuer-url <OAUTH_ISSUER_URL>        OIDC issuer URL (defaults to <https://{host>})
      --oauth-client-id <OAUTH_CLIENT_ID>          OAuth client ID (defaults to "rattler")
      --oauth-client-secret <OAUTH_CLIENT_SECRET>  OAuth client secret (for confidential clients)
      --oauth-flow <OAUTH_FLOW>                    OAuth flow: auto (default), auth-code, device-code [possible values: auto, auth-code,
                                                   device-code]
      --oauth-scope <OAUTH_SCOPES>                 Additional OAuth scopes to request (repeatable)
      --oauth-redirect-uri <OAUTH_REDIRECT_URI>    OAuth redirect URI (defaults to a random localhost port). Set this when the OAuth client on
                                                   the `IdP` side is registered with a specific redirect URI such as
                                                   `http://127.0.0.1:8000/auth/oidc`

Global Options:
  -h, --help           Display help information
  -v, --verbose...     Increase logging verbosity (-v for warnings, -vv for info, -vvv for debug, -vvvv for trace)
  -q, --quiet...       Decrease logging verbosity (quiet mode)
      --color <COLOR>  Whether the log needs to be colored [env: PIXI_COLOR=] [default: auto] [possible values: always, never, auto]
      --no-progress    Hide all progress bars, always turned on if stderr is not a terminal [env: PIXI_NO_PROGRESS=]
```

## OAuth/OIDC authentication

The simplest way to authenticate against prefix.dev is:

```shell
pixi auth login prefix.dev
```

A browser tab opens, you log in to prefix.dev, and Pixi stores the credentials in your system keychain. Every later `pixi install`, `pixi upload`, or `pixi publish` reuses them automatically, and Pixi refreshes the token in the background before it expires.

What this login allows

Pixi requests the rights to access your profile, read from channels, and upload your packages. This covers the basics commands like `pixi install`, `pixi upload` or `pixi publish` to work out of the box.

### Choosing an OAuth flow

`pixi auth login` defaults to `--oauth-flow auto`, which picks at runtime:

- **Authorization Code with PKCE** when you're on a desktop machine with a working browser. Pixi opens a browser tab, you sign in on prefix.dev, and a local callback page completes the exchange.
- **Device Code** when Pixi can't open a browser for you. Pixi prints a short user code and a URL; you open the URL on any device, paste the code, and Pixi receives the authentication. This is the common case for SSH sessions, headless servers, container shells, and similar environments without a browser.

You can force a specific flow if `auto` picks wrong for your environment:

```shell
pixi auth login prefix.dev --oauth-flow device-code
```

### Other OIDC providers

For self-hosted OIDC providers (or any host where Pixi doesn't ship built-in defaults), pass the configuration explicitly:

```shell
pixi auth login my.private.host \
    --oauth \
    --oauth-issuer-url https://idp.example.com \
    --oauth-client-id my-cli-client \
    --oauth-scope openid --oauth-scope channel:read
```

## Token authentication

If you prefer to manage tokens yourself (e.g. in CI, where browser login isn't an option), you can use a standard "Bearer Token" authentication

```shell
pixi auth login prefix.dev --token pfx_jj8WDzvnuTHEGdAhwRZMC1Ag8gSto8
```

Bearer tokens are sent with every request as an `Authorization: Bearer <TOKEN>` header.

## anaconda.org / quetz

```shell
pixi auth login anaconda.org --conda-token xy-72b914cc-c105-4ec7-a969-ab21d23480ed
```

The conda-token is embedded in the URL path: `conda.anaconda.org/t/<TOKEN>/conda-forge/linux-64/...`. Quetz servers accept the same scheme.

## Basic HTTP authentication

```shell
pixi auth login myserver.com --username user --password password
```

This is the equivalent of `http://user:password@myserver.com/...` and is the typical choice for self-hosted channels behind an NGINX/Apache reverse proxy with HTTP basic auth.

## Examples

Below are the existing per-method examples, kept for quick reference.

Login to prefix.dev with a manual token:

```shell
pixi auth login prefix.dev --token pfx_jj8WDzvnuTHEGdAhwRZMC1Ag8gSto8
```

Login to anaconda.org:

```shell
pixi auth login anaconda.org --conda-token xy-72b914cc-c105-4ec7-a969-ab21d23480ed
```

Login to a basic HTTP secured server:

```shell
pixi auth login myserver.com --username user --password password
```

Login to an S3 bucket:

```shell
pixi auth login s3://my-bucket --s3-access-key-id <access-key-id> --s3-secret-access-key <secret-access-key>
# if your key uses a session token, you can also use:
pixi auth login s3://my-bucket --s3-access-key-id <access-key-id> --s3-secret-access-key <secret-access-key> --s3-session-token <session-token>
```

Note

S3 authentication is also supported through AWS's typical `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY` environment variables, see the [S3 section](../s3/) for more details.

## Where does Pixi store the authentication information?

On Windows, the credentials are stored in the "credentials manager". Searching for `rattler` (the underlying library Pixi uses) you should find any credentials stored by Pixi (or other rattler-based programs).

On macOS, the passwords are stored in the keychain. To access the password, you can use the `Keychain Access` program that comes pre-installed on macOS. Searching for `rattler` (the underlying library Pixi uses) you should find any credentials stored by Pixi (or other rattler-based programs).

On Linux, one can use `GNOME Keyring` (or just Keyring) to access credentials that are securely stored by `libsecret`. Searching for `rattler` should list all the credentials stored by Pixi and other rattler-based programs.

## Fallback storage

If you run on a server with none of the aforementioned keychains available, then Pixi falls back to store the credentials in an *insecure* JSON file. This JSON file is located at `~/.rattler/credentials.json` and contains the credentials.

## Override the authentication storage

You can use the `RATTLER_AUTH_FILE` environment variable to override the default location of the credentials file. When this environment variable is set, it provides the only source of authentication data that is used by pixi.

E.g.

```bash
export RATTLER_AUTH_FILE=$HOME/credentials.json
# You can also specify the file in the command line
pixi global install --auth-file $HOME/credentials.json ...
```

Note

`RATTLER_AUTH_FILE` has higher precedence than the CLI argument.

The JSON should follow the following format:

```json
{
    "*.prefix.dev": {
        "BearerToken": "your_token"
    },
    "otherhost.com": {
        "BasicHTTP": {
            "username": "your_username",
            "password": "your_password"
        }
    },
    "conda.anaconda.org": {
        "CondaToken": "your_token"
    },
    "s3://my-bucket": {
        "S3Credentials": {
            "access_key_id": "my-access-key-id",
            "secret_access_key": "my-secret-access-key",
            "session_token": null
        }
    }
}
```

Note: if you use a wildcard in the host, any subdomain will match (e.g. `*.prefix.dev` also matches `repo.prefix.dev`).

Lastly you can set the authentication override file in the [global configuration file](../../reference/pixi_configuration/).

## PyPI authentication

Currently, we support the following methods for authenticating against PyPI:

1. [keyring](https://pypi.org/project/keyring/) authentication.
1. `.netrc` file authentication.

We want to add more methods in the future, so if you have a specific method you would like to see, please let us know.

### Keyring authentication

Currently, Pixi supports the uv method of authentication through the python [keyring](https://pypi.org/project/keyring/) library.

#### Installing keyring

To install keyring you can use `pixi global install`:

```shell
pixi global install keyring
```

```shell
pixi global install keyring --with keyrings.google-artifactregistry-auth
```

```shell
pixi global install keyring --with keyrings.artifacts
```

```shell
pixi global install keyring --with keyrings.codeartifact
```

For other registries, you will need to adapt these instructions to add the right keyring backend.

#### Configuring your workspace to use keyring

Use keyring to store your credentials e.g:

```shell
keyring set https://my-index/simple your_username
# prompt will appear for your password
```

Add the following configuration to your Pixi manifest, making sure to include `your_username@` in the URL of the registry:

```toml
[pypi-options]
index-url = "https://your_username@custom-registry.com/simple"
```

After making sure you are logged in, for instance by running `gcloud auth login`, add the following configuration to your Pixi manifest:

```toml
[pypi-options]
extra-index-urls = ["https://oauth2accesstoken@<location>-python.pkg.dev/<project>/<repository>/simple"]
```

Note

To find this URL more easily, you can use the `gcloud` command:

```shell
gcloud artifacts print-settings python --project=<project> --repository=<repository> --location=<location>
```

After following the [`keyrings.artifacts` instructions](https://github.com/jslorrma/keyrings.artifacts?tab=readme-ov-file#usage) and making sure that keyring works correctly, add the following configuration to your Pixi manifest:

```toml
[pypi-options]
extra-index-urls = ["https://VssSessionToken@pkgs.dev.azure.com/{organization}/{project}/_packaging/{feed}/pypi/simple/"]
```

Ensure you are logged in e.g via `aws sso login` and add the following configuration to your Pixi manifest:

```toml
[pypi-options]
extra-index-urls = ["https://aws@<your-domain>-<your-account>.d.codeartifact.<your-region>.amazonaws.com/pypi/<your-repository>/simple/"]
```

#### Installing your environment

Either configure your [Global Config](../../reference/pixi_configuration/#pypi-config), or use the flag `--pypi-keyring-provider` which can either be set to `subprocess` (activated) or `disabled`:

```shell
# From an existing pixi workspace
pixi install --pypi-keyring-provider subprocess
```

### `.netrc` file

`pixi` allows you to access private registries securely by authenticating with credentials stored in a `.netrc` file.

- The `.netrc` file can be stored in your home directory (`$HOME/.netrc` for Unix-like systems)
- or in the user profile directory on Windows (`%HOME%\_netrc`).
- You can also set up a different location for it using the `NETRC` variable (`export NETRC=/my/custom/location/.netrc`). e.g `export NETRC=/my/custom/location/.netrc pixi install`

In the `.netrc` file, you store authentication details like this:

```sh
machine registry-name
login admin
password admin
```

For more details, you can access the [.netrc docs](https://www.ibm.com/docs/en/aix/7.2?topic=formats-netrc-file-format-tcpip).
