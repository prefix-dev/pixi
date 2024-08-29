---
part: pixi
title: Authenticate pixi with a server
description: Authenticate pixi to access private channels
---

You can authenticate pixi with a server like prefix.dev, a private quetz instance or anaconda.org.
Different servers use different authentication methods.
In this documentation page, we detail how you can authenticate against the different servers and where the authentication information is stored.

```shell
Usage: pixi auth login [OPTIONS] <HOST>

Arguments:
  <HOST>  The host to authenticate with (e.g. repo.prefix.dev)

Options:
      --token <TOKEN>              The token to use (for authentication with prefix.dev)
      --username <USERNAME>        The username to use (for basic HTTP authentication)
      --password <PASSWORD>        The password to use (for basic HTTP authentication)
      --conda-token <CONDA_TOKEN>  The token to use on anaconda.org / quetz authentication
  -v, --verbose...                 More output per occurrence
  -q, --quiet...                   Less output per occurrence
  -h, --help                       Print help
```

The different options are "token", "conda-token" and "username + password".

The token variant implements a standard "Bearer Token" authentication as is used on the prefix.dev platform.
A Bearer Token is sent with every request as an additional header of the form `Authentication: Bearer <TOKEN>`.

The conda-token option is used on anaconda.org and can be used with a quetz server. With this option, the token is sent as part of the URL following this scheme: `conda.anaconda.org/t/<TOKEN>/conda-forge/linux-64/...`.

The last option, username & password, are used for "Basic HTTP Authentication". This is the equivalent of adding `http://user:password@myserver.com/...`. This authentication method can be configured quite easily with a reverse NGinx or Apache server and is thus commonly used in self-hosted systems.

## Examples

Login to prefix.dev:

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

## Where does pixi store the authentication information?

The storage location for the authentication information is system-dependent. By default, pixi tries to use the keychain to store this sensitive information securely on your machine.

On Windows, the credentials are stored in the "credentials manager". Searching for `rattler` (the underlying library pixi uses) you should find any credentials stored by pixi (or other rattler-based programs).

On macOS, the passwords are stored in the keychain. To access the password, you can use the `Keychain Access` program that comes pre-installed on macOS. Searching for `rattler` (the underlying library pixi uses) you should find any credentials stored by pixi (or other rattler-based programs).

On Linux, one can use `GNOME Keyring` (or just Keyring) to access credentials that are securely stored by `libsecret`. Searching for `rattler` should list all the credentials stored by pixi and other rattler-based programs.

## Fallback storage

If you run on a server with none of the aforementioned keychains available, then pixi falls back to store the credentials in an _insecure_ JSON file.
This JSON file is located at `~/.rattler/credentials.json` and contains the credentials.

## Override the authentication storage

You can use the `RATTLER_AUTH_FILE` environment variable to override the default location of the credentials file.
When this environment variable is set, it provides the only source of authentication data that is used by pixi.

E.g.

```bash
export RATTLER_AUTH_FILE=$HOME/credentials.json
# You can also specify the file in the command line
pixi global install --auth-file $HOME/credentials.json ...
```

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
    }
}
```

Note: if you use a wildcard in the host, any subdomain will match (e.g. `*.prefix.dev` also matches `repo.prefix.dev`).

Lastly you can set the authentication override file in the [global configuration file](./../reference/pixi_configuration.md).

## PyPI authentication
Currently, we support the following methods for authenticating against PyPI:

1. [keyring](https://pypi.org/project/keyring/) authentication.
2. `.netrc` file authentication.

We want to add more methods in the future, so if you have a specific method you would like to see, please let us know.

### Keyring authentication

Currently, pixi supports the uv method of authentication through the python [keyring](https://pypi.org/project/keyring/) library.
To enable this use the CLI flag `--pypi-keyring-provider` which can either be set to `subprocess` (activated) or `disabled`.

```shell
# From an existing pixi project
pixi install --pypi-keyring-provider subprocess
```

This option can also be set in the global configuration file under [pypi-config](./../reference/pixi_configuration.md#pypi-configuration).

#### Installing keyring
To install keyring you can use pixi global install:

Either use:

```shell
pixi global install keyring
```
??? warning "GCP and other backends"
    The downside of this method is currently, because you cannot inject into a pixi global environment just yet, that installing different keyring backends is not possible. This allows only the default keyring backend to be used.
    Give the [issue](https://github.com/prefix-dev/pixi/issues/342) a 👍 up if you would like to see inject as a feature.

Or alternatively, you can install keyring using pipx:

```shell
# Install pipx if you haven't already
pixi global install pipx
pipx install keyring

# For Google Artifact Registry, also install and initialize its keyring backend.
# Inject this into the pipx environment
pipx inject keyring keyrings.google-artifactregistry-auth --index-url https://pypi.org/simple
gcloud auth login
```

#### Using keyring with Basic Auth
Use keyring to store your credentials e.g:

```shell
keyring set https://my-index/simple your_username
# prompt will appear for your password
```

##### Configuration
Make sure to include `username@` in the URL of the registry.
An example of this would be:

```toml
[pypi-options]
index-url = "https://username@custom-registry.com/simple"
```

#### GCP
For Google Artifact Registry, you can use the Google Cloud SDK to authenticate.
Make sure to have run `gcloud auth login` before using pixi.
Another thing to note is that you need to add `oauth2accesstoken` to the URL of the registry.
An example of this would be:

##### Configuration

```toml
# rest of the pixi.toml
#
# Add's the following options to the default feature
[pypi-options]
extra-index-urls = ["https://oauth2accesstoken@<location>-python.pkg.dev/<project>/<repository>/simple"]
```

!!!Note
    Include the `/simple` at the end, replace the `<location>` etc. with your project and repository and location.
To find this URL more easily, you can use the `gcloud` command:

```shell
gcloud artifacts print-settings python --project=<project> --repository=<repository> --location=<location>
```

### Azure DevOps
Similarly for Azure DevOps, you can use the Azure keyring backend for authentication.
The backend, along with installation instructions can be found at [keyring.artifacts](https://github.com/jslorrma/keyrings.artifacts).

After following the instructions and making sure that keyring works correctly, you can use the following configuration:

##### Configuration
```toml
# rest of the pixi.toml
#
# Adds the following options to the default feature
[pypi-options]
extra-index-urls = ["https://VssSessionToken@pkgs.dev.azure.com/{organization}/{project}/_packaging/{feed}/pypi/simple/"]
```
This should allow for getting packages from the Azure DevOps artifact registry.


#### Installing your environment
To actually install either configure your [Global Config](../reference/pixi_configuration.md#pypi-config), or use the flag:
```shell
pixi install --pypi-keyring-provider subprocess
```


### `.netrc` file

`pixi` allows you to access private registries securely by authenticating with credentials stored in a `.netrc` file.

- The `.netrc` file can be stored in your home directory (`$HOME/.netrc` for Unix-like systems)
- or in the user profile directory on Windows (`%HOME%\_netrc`).
- You can also set up a different location for it using the `NETRC` variable (`export NETRC=/my/custom/location/.netrc`).
  e.g `export NETRC=/my/custom/location/.netrc pixi install`

In the `.netrc` file, you store authentication details like this:

```sh
machine registry-name
login admin
password admin
```
For more details, you can access the [.netrc docs](https://www.ibm.com/docs/en/aix/7.2?topic=formats-netrc-file-format-tcpip).
