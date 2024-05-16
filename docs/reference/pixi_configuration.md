# The configuration of pixi itself

Apart from the [project specific configuration](../reference/project_configuration.md) pixi supports configuration options which are not required for the project to work but are local to the machine.
The configuration is loaded in the following order:


=== "Linux"

    | **Priority** | **Location**                                                           | **Comments**                                                                       |
    |--------------|------------------------------------------------------------------------|------------------------------------------------------------------------------------|
    | 1            | `/etc/pixi/config.toml`                                                | System-wide configuration                                                          |
    | 2            | `$XDG_CONFIG_HOME/pixi/config.toml`                                    | XDG compliant user-specific configuration                                          |
    | 3            | `$HOME/.config/pixi/config.toml`                                       | User-specific configuration                                                        |
    | 4            | `$PIXI_HOME/config.toml`                                               | Global configuration in the user home directory. `PIXI_HOME` defaults to `~/.pixi` |
    | 5            | `your_project/.pixi/config.toml`                                       | Project-specific configuration                                                     |
    | 6            | Command line arguments (`--tls-no-verify`, `--change-ps1=false`, etc.) | Configuration via command line arguments                                           |

=== "macOS"

    | **Priority** | **Location**                                                           | **Comments**                                                                       |
    |--------------|------------------------------------------------------------------------|------------------------------------------------------------------------------------|
    | 1            | `/etc/pixi/config.toml`                                                | System-wide configuration                                                          |
    | 2            | `$XDG_CONFIG_HOME/pixi/config.toml`                                    | XDG compliant user-specific configuration                                          |
    | 3            | `$HOME/Library/Application Support/pixi/config.toml`                   | User-specific configuration                                                        |
    | 4            | `$PIXI_HOME/config.toml`                                               | Global configuration in the user home directory. `PIXI_HOME` defaults to `~/.pixi` |
    | 5            | `your_project/.pixi/config.toml`                                       | Project-specific configuration                                                     |
    | 6            | Command line arguments (`--tls-no-verify`, `--change-ps1=false`, etc.) | Configuration via command line arguments                                           |

=== "Windows"

    | **Priority** | **Location**                                                           | **Comments**                                                                                   |
    |--------------|------------------------------------------------------------------------|------------------------------------------------------------------------------------------------|
    | 1            | `C:\ProgramData\pixi\config.toml`                                      | System-wide configuration                                                                      |
    | 2            | `%APPDATA%\pixi\config.toml`                                           | User-specific configuration                                                                    |
    | 3            | `$PIXI_HOME\config.toml`                                               | Global configuration in the user home directory. `PIXI_HOME` defaults to `%USERPROFILE%/.pixi` |
    | 4            | `your_project\.pixi\config.toml`                                       | Project-specific configuration                                                                 |
    | 5            | Command line arguments (`--tls-no-verify`, `--change-ps1=false`, etc.) | Configuration via command line arguments                                                       |

!!! note
    The highest priority wins. If a configuration file is found in a higher priority location, the lower priority locations are overwritten.


!!! note
    To find the locations where `pixi` looks for configuration files, run
    `pixi` with `-v` or `--verbose`.

## Reference

??? info "Casing In Configuration"
    In versions of pixi `0.20.1` and older the global configuration used snake_case
    we've changed to `kebab-case` for consistency with the rest of the configuration.
    But we still support the old `snake_case` configuration, for older configuration options.
    These are:

    - `default_channels`
    - `change_ps1`
    - `tls_no_verify`
    - `authentication_override_file`
    - `mirrors` and sub-options
    - `repodata-config` and sub-options

The following reference describes all available configuration options.

### `default-channels`

The default channels to select when running `pixi init` or `pixi global install`.
This defaults to only conda-forge.
```toml title="config.toml"
default-channels = ["conda-forge"]
```

### `change-ps1`

When set to false, the `(pixi)` prefix in the shell prompt is removed.
This applies to the `pixi shell` subcommand.
You can override this from the CLI with `--change-ps1`.

```toml title="config.toml"
change-ps1 = true
```

### `tls-no-verify`
When set to true, the TLS certificates are not verified.

!!! warning

    This is a security risk and should only be used for testing purposes or internal networks.

You can override this from the CLI with `--tls-no-verify`.

```toml title="config.toml"
tls-no-verify = false
```

### `authentication-override-file`
Override from where the authentication information is loaded.
Usually, we try to use the keyring to load authentication data from, and only use a JSON
file as a fallback. This option allows you to force the use of a JSON file.
Read more in the authentication section.
```toml title="config.toml"
authentication-override-file = "/path/to/your/override.json"
```

### `target-environment-directory`
The directory where pixi stores the project environments, what would normally be placed in the `.pixi` folder in a project's root.
It doesn't affect the environments built for `pixi global`. 
The location of environments created for a `pixi global` installation can be controlled using the `PIXI_HOME` environment variable.
This is not recommended to change, but can be useful in some cases.

- Forcing the installation on a specific filesystem/drive
- Network hosted projects, but local environments
- Let the system-administrator have more control over all environments on a system.

```toml title="config.toml"
target-environment-diretory = "/opt/pixi/envs"
```

### `mirrors`
Configuration for conda channel-mirrors, more info [below](#mirror-configuration).

```toml title="config.toml"
[mirrors]
# redirect all requests for conda-forge to the prefix.dev mirror
"https://conda.anaconda.org/conda-forge" = [
    "https://prefix.dev/conda-forge"
]

# redirect all requests for bioconda to one of the three listed mirrors
# Note: for repodata we try the first mirror first.
"https://conda.anaconda.org/bioconda" = [
    "https://conda.anaconda.org/bioconda",
    # OCI registries are also supported
    "oci://ghcr.io/channel-mirrors/bioconda",
    "https://prefix.dev/bioconda",
]
```

### `repodata-config`
Configuration for repodata fetching.
```toml title="config.toml"
[repodata-config]
# disable fetching of jlap, bz2 or zstd repodata files.
# This should only be used for specific old versions of artifactory and other non-compliant
# servers.
disable-jlap = true  # don't try to download repodata.jlap
disable-bzip2 = true # don't try to download repodata.json.bz2
disable-zstd = true  # don't try to download repodata.json.zst
```

### `pypi-config`
To setup a certain number of defaults for the usage of PyPI registries. You can use the following configuration options:

- `index-url`: The default index URL to use for PyPI packages. This will be added to a manifest file on a `pixi init`.
- `extra-index-urls`: A list of additional URLs to use for PyPI packages. This will be added to a manifest file on a `pixi init`.
- `keyring-provider`: Allows the use of the [keyring](https://pypi.org/project/keyring/) python package to store and retrieve credentials.

```toml title="config.toml"
[pypi-config]
# Main index url
index-url = "https://pypi.org/simple"
# list of additional urls
extra-index-urls = ["https://pypi.org/simple2"]
# can be "subprocess" or "disabled"
keyring-provider = "subprocess"
```

!!! Note "`index-url` and `extra-index-urls` are *not* globals"
    Unlike pip, these settings, with the exception of `keyring-provider` will only modify the `pixi.toml`/`pyproject.toml` file and are not globally interpreted when not present in the manifest.
    This is because we want to keep the manifest file as complete and reproducible as possible.

## Mirror configuration

You can configure mirrors for conda channels. We expect that mirrors are exact
copies of the original channel. The implementation will look for the mirror key
(a URL) in the `mirrors` section of the configuration file and replace the
original URL with the mirror URL.

To also include the original URL, you have to repeat it in the list of mirrors.

The mirrors are prioritized based on the order of the list. We attempt to fetch
the repodata (the most important file) from the first mirror in the list. The
repodata contains all the SHA256 hashes of the individual packages, so it is
important to get this file from a trusted source.

You can also specify mirrors for an entire "host", e.g.

```toml title="config.toml"
[mirrors]
"https://conda.anaconda.org" = [
    "https://prefix.dev/"
]
```

This will forward all request to channels on anaconda.org to prefix.dev.
Channels that are not currently mirrored on prefix.dev will fail in the above example.

### OCI Mirrors

You can also specify mirrors on the OCI registry. There is a public mirror on
the Github container registry (ghcr.io) that is maintained by the conda-forge
team. You can use it like this:

```toml title="config.toml"
[mirrors]
"https://conda.anaconda.org/conda-forge" = [
    "oci://ghcr.io/channel-mirrors/conda-forge"
]
```

The GHCR mirror also contains `bioconda` packages. You can search the [available
packages on Github](https://github.com/orgs/channel-mirrors/packages).
