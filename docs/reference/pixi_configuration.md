# The Configuration of Pixi Itself

Apart from the [project specific configuration](../reference/pixi_manifest.md) Pixi supports configuration options which
are not required for the project to work but are local to the machine.
The configuration is loaded in the following order:

=== "Linux"

    | **Priority** | **Location**                                                           | **Comments**                                          |
    |--------------|------------------------------------------------------------------------|-------------------------------------------------------|
    | 7            | Command line arguments (`--tls-no-verify`, `--change-ps1=false`, etc.) | Configuration via command line arguments              |
    | 6            | `your_project/.pixi/config.toml`                                       | Project-specific configuration                        |
    | 5            | `$PIXI_HOME/config.toml`                                               | Global configuration in `PIXI_HOME`.                  |
    | 4            | `$HOME/.pixi/config.toml`                                              | Global configuration in the user home directory.      |
    | 3            | `$XDG_CONFIG_HOME/pixi/config.toml`                                    | XDG compliant user-specific configuration             |
    | 2            | `$HOME/.config/pixi/config.toml`                                       | User-specific configuration                           |
    | 1            | `/etc/pixi/config.toml`                                                | System-wide configuration                             |

=== "macOS"

    | **Priority** | **Location**                                                           | **Comments**                                          |
    |--------------|------------------------------------------------------------------------|-------------------------------------------------------|
    | 7            | Command line arguments (`--tls-no-verify`, `--change-ps1=false`, etc.) | Configuration via command line arguments              |
    | 6            | `your_project/.pixi/config.toml`                                       | Project-specific configuration                        |
    | 5            | `$PIXI_HOME/config.toml`                                               | Global configuration in `PIXI_HOME`.                  |
    | 4            | `$HOME/.pixi/config.toml`                                              | Global configuration in the user home directory.      |
    | 3            | `$HOME/Library/Application Support/pixi/config.toml`                   | User-specific configuration                           |
    | 2            | `$XDG_CONFIG_HOME/pixi/config.toml`                                    | XDG compliant user-specific configuration             |
    | 1            | `/etc/pixi/config.toml`                                                | System-wide configuration                             |

=== "Windows"

    | **Priority** | **Location**                                                           | **Comments**                                          |
    |--------------|------------------------------------------------------------------------|-------------------------------------------------------|
    | 6            | Command line arguments (`--tls-no-verify`, `--change-ps1=false`, etc.) | Configuration via command line arguments              |
    | 5            | `your_project\.pixi\config.toml`                                       | Project-specific configuration                        |
    | 4            | `%PIXI_HOME%\config.toml`                                              | Global configuration in `PIXI_HOME`.                  |
    | 3            | `%USERPROFILE%\.pixi\config.toml`                                      | Global configuration in the user home directory.      |
    | 2            | `%APPDATA%\pixi\config.toml`                                           | User-specific configuration                           |
    | 1            | `C:\ProgramData\pixi\config.toml`                                      | System-wide configuration                             |

!!! note
The highest priority wins. If a configuration file is found in a higher priority location, the values from the
configuration read from lower priority locations are overwritten.

!!! note
To find the locations where `pixi` looks for configuration files, run
`pixi info -vvv`.

## Configuration options

??? info "Naming convention in configuration"
In Pixi `0.20.1` and older the global configuration options used `snake_case` which
we've changed to `kebab-case` for consistency with the rest of the configuration.
For backwards compatibility, the following configuration options can still be written
in `snake_case`:

    - `default_channels`
    - `change_ps1`
    - `tls_no_verify`
    - `authentication_override_file`
    - `mirrors` and its sub-options
    - `repodata_config` and its sub-options

The following reference describes all available configuration options.

### `default-channels`

The default channels to select when running `pixi init` or `pixi global install`.
This defaults to only conda-forge.

```toml title="config.toml"
--8<-- "docs/source_files/pixi_config_tomls/main_config.toml:default-channels"
```

!!! note
The `default-channels` are only used when initializing a new project. Once initialized the `channels` are used from the
project manifest.

### `shell`

- `change-ps1`:  When set to `false`, the `(pixi)` prefix in the shell prompt is removed.
  This applies to the `pixi shell` subcommand.
  You can override this from the CLI with `--change-ps1`.
- `force-activate`: When set to `true` the re-activation of the environment will always happen.
  This is used in combination with the [`experimental`](#experimental) feature `use-environment-activation-cache`.
- `source-completion-scripts`: When set to `false`, Pixi will not source the autocompletion scripts of the environment
  when going into the shell.

```toml title="config.toml"
--8<-- "docs/source_files/pixi_config_tomls/main_config.toml:shell"
```

### `tls-no-verify`

When set to true, the TLS certificates are not verified.

!!! warning

    This is a security risk and should only be used for testing purposes or internal networks.

You can override this from the CLI with `--tls-no-verify`.

```toml title="config.toml"
--8<-- "docs/source_files/pixi_config_tomls/main_config.toml:tls-no-verify"
```

### `authentication-override-file`

Override from where the authentication information is loaded.
Usually, we try to use the keyring to load authentication data from, and only use a JSON
file as a fallback. This option allows you to force the use of a JSON file.
Read more in the authentication section.

```toml title="config.toml"
--8<-- "docs/source_files/pixi_config_tomls/main_config.toml:authentication-override-file"
```

### `detached-environments`

The directory where Pixi stores the project environments, what would normally be placed in the `.pixi/envs` folder in a
project's root.
It doesn't affect the environments built for `pixi global`.
The location of environments created for a `pixi global` installation can be controlled using the `PIXI_HOME`
environment variable.
!!! warning
We recommend against using this because any environment created for a project is no longer placed in the same folder as
the project.
This creates a disconnect between the project and its environments and manual cleanup of the environments is required
when deleting the project.

    However, in some cases, this option can still be very useful, for instance to:

    - force the installation on a specific filesystem/drive.
    - install environments locally but keep the project on a network drive.
    - let a system-administrator have more control over all environments on a system.

This field can consist of two types of input.

- A boolean value, `true` or `false`, which will enable or disable the feature respectively. (not `"true"` or `"false"`,
  this is read as `false`)
- A string value, which will be the absolute path to the directory where the environments will be stored.

```toml title="config.toml"
--8<-- "docs/source_files/pixi_config_tomls/main_config.toml:detached-environments"
```

or:

```toml title="config.toml"
--8<-- "docs/source_files/pixi_config_tomls/detached_environments_path_config.toml:detached-environments-path"
```

The environments will be stored in the [cache directory](../workspace/environment.md#caching-packages) when this option
is `true`.
When you specify a custom path the environments will be stored in that directory.

The resulting directory structure will look like this:

```shell
/opt/pixi/envs
├── pixi-6837172896226367631
│   └── envs
└── NAME_OF_PROJECT-HASH_OF_ORIGINAL_PATH
    ├── envs # the runnable environments
    └── solve-group-envs # If there are solve groups

```

### `pinning-strategy`

The strategy to use for pinning dependencies when running `pixi add`.
The default is `semver` but you can set the following:

- `no-pin`: No pinning, resulting in an unconstraint dependency. `*`
- `semver`: Pinning to the latest version that satisfies the semver constraint. Resulting in a pin to major for most
  versions and to minor for `v0` versions.
- `exact-version`: Pinning to the exact version, `1.2.3` -> `==1.2.3`.
- `major`: Pinning to the major version, `1.2.3` -> `>=1.2.3, <2`.
- `minor`: Pinning to the minor version, `1.2.3` -> `>=1.2.3, <1.3`.
- `latest-up`: Pinning to the latest version, `1.2.3` -> `>=1.2.3`.

```toml title="config.toml"
--8<-- "docs/source_files/pixi_config_tomls/main_config.toml:pinning-strategy"
```

### `mirrors`

Configuration for conda channel-mirrors, more info [below](#mirror-configuration).

```toml title="config.toml"
--8<-- "docs/source_files/pixi_config_tomls/main_config.toml:mirrors"
```

### `proxy-config`

`pixi` respects the proxy environments such as `https_proxy` with the highest priority.
Also we can set the proxies in `proxy-config` table of the `pixi` config, which affect all the pixi
network actions, such as resolve, self-update, download, etc.
Now `proxy-config` table supports the following options:

- `https` and `http`: The proxy url for https:// or http:// url, works like `https_proxy` and `http_proxy` environments.
- `non-proxy-hosts`: A list of domains which should bypass proxies.


```toml title="config.toml"
--8<-- "docs/source_files/pixi_config_tomls/main_config.toml:proxy-config"
```


### `repodata-config`

Configuration for repodata fetching.

```toml title="config.toml"
--8<-- "docs/source_files/pixi_config_tomls/main_config.toml:repodata-config"
```

The above settings can be overridden on a per-channel basis by specifying a channel prefix in the configuration.

```toml title="config.toml"
--8<-- "docs/source_files/pixi_config_tomls/main_config.toml:prefix-repodata-config"
```

### `pypi-config`

To setup a certain number of defaults for the usage of PyPI registries. You can use the following configuration options:

- `index-url`: The default index URL to use for PyPI packages. This will be added to a manifest file on a `pixi init`.
- `extra-index-urls`: A list of additional URLs to use for PyPI packages. This will be added to a manifest file on a
  `pixi init`.
- `keyring-provider`: Allows the use of the [keyring](https://pypi.org/project/keyring/) python package to store and
  retrieve credentials.
- `allow-insecure-host`: Allow insecure connections to host.

```toml title="config.toml"
--8<-- "docs/source_files/pixi_config_tomls/main_config.toml:pypi-config"
```

!!! Note "`index-url` and `extra-index-urls` are *not* globals"
Unlike pip, these settings, with the exception of `keyring-provider` will only modify the `pixi.toml`/`pyproject.toml`
file and are not globally interpreted when not present in the manifest.
This is because we want to keep the manifest file as complete and reproducible as possible.

### `s3-options`

Configuration for S3 authentication. This will lead to Pixi not using AWS's default credentials but instead use the
credentials from the Pixi authentication storage, see the [S3 section](../deployment/s3.md) for more information.

```toml title="config.toml"
--8<-- "docs/source_files/pixi_config_tomls/main_config.toml:s3-options"
```

### `concurrency`

Configure multiple settings to limit or extend the concurrency of pixi.

```toml title="config.toml"
--8<-- "docs/source_files/pixi_config_tomls/main_config.toml:concurrency"
```

Set them through the CLI with:

```shell
pixi config set concurrency.solves 1
pixi config set concurrency.downloads 12
```

### `run-post-link-scripts`

Configure whether pixi should execute `post-link` and `pre-unlink` scripts or not.

Some packages contain post-link scripts (`bat` or `sh` files) that are executed after a package is installed. We deem
these scripts as insecure because they can contain arbitrary code that is executed on the user's machine without the
user's consent. By default, the value of `run-post-link-scripts` is set to `false` which prevents the execution of these
scripts.

However, you can opt-in on a global (or project) basis by setting the value to `insecure` (e.g. by running
`pixi config set --local run-post-link-scripts insecure`).

In the future we are planning to add a `sandbox` mode to execute these scripts in a controlled environment.

```toml title="config.toml"
--8<-- "docs/source_files/pixi_config_tomls/main_config.toml:run-post-link-scripts"
```

### `tool-platform`

Defines the platform used when installing "tools" like build backends. By default pixi uses the platform of the current
system, but you can override it to use a different platform. This can be useful if you are running pixi on an
architecture for which there is fewer support for certain build backends.

```toml title="config.toml"
--8<-- "docs/source_files/pixi_config_tomls/main_config.toml:tool-platform"
```

!!! Note "Virtual packages"
The virtual packages for the tool platform are detected from the current system. If the tool platform is for a different
operating system than the current system, no virtual packages will be used.

## Experimental

This allows the user to set specific experimental features that are not yet stable.

Please write a GitHub issue and add the flag `experimental` to the issue if you find issues with the feature you
activated.

### Caching environment activations

Turn this feature on from configuration with the following command:

```shell
# For all your projects
pixi config set experimental.use-environment-activation-cache true --global

# For a specific project
pixi config set experimental.use-environment-activation-cache true --local
```

This will cache the environment activation in the `.pixi/activation-env-v0` folder in the project root.
It will create a json file for each environment that is activated, and it will be used to activate the environment in
the future.

```bash
> tree .pixi/activation-env-v0/
.pixi/activation-env-v0/
├── activation_default.json
└── activation_lint.json

> cat  .pixi/activation-env-v0/activation_lint.json
{"hash":"8d8344e0751d377a","environment_variables":{<ENVIRONMENT_VARIABLES_USED_IN_ACTIVATION>}}
```

- The `hash` is a hash of the data on that environment in the `pixi.lock`, plus some important information on the
  environment activation.
  Like `[activation.scripts]` and `[activation.env]` from the manifest file.
- The `environment_variables` are the environment variables that are set when activating the environment.

You can ignore the cache by running:

```
pixi run/shell/shell-hook --force-activate
```

Set the configuration with:

```toml title="config.toml"
--8<-- "docs/source_files/pixi_config_tomls/main_config.toml:experimental"
```

!!! note "Why is this experimental?"
This feature is experimental because the cache invalidation is very tricky,
and we don't want to disturb users that are not affected by activation times.

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
--8<-- "docs/source_files/pixi_config_tomls/mirror_prefix_config.toml:mirrors"
```

This will forward all request to channels on anaconda.org to prefix.dev.
Channels that are not currently mirrored on prefix.dev will fail in the above example.
You can override the behavior for specific channels (like conda-forge's label channels)
by providing a longer prefix that points to itself.

### OCI Mirrors

You can also specify mirrors on the OCI registry. There is a public mirror on
the Github container registry (ghcr.io) that is maintained by the conda-forge
team. You can use it like this:

```toml title="config.toml"
--8<-- "docs/source_files/pixi_config_tomls/oci_config.toml:oci-mirrors"
```

The GHCR mirror also contains `bioconda` packages. You can search the [available
packages on Github](https://github.com/orgs/channel-mirrors/packages).

### Mirrors for PyPi resolving and PyPi package downloading

The mirrors also affect the PyPi resolving and downloading steps of `uv`.
You can set mirrors for the main pypi index and extra indices. But the base part of
downloading urls may vary from their index urls. For example, the default pypi index
is `https://pypi.org/simple`, and the package downloading urls are in the form of
`https://files.pythonhosted.org/packages/<h1>/<h2>/<hash>/<package>.whl`. You need two mirror
config entries for `https://pypi.org/simple` and `https://files.pythonhosted.org/packages`.
