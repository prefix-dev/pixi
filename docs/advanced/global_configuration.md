# Global configuration in pixi

Pixi supports some global configuration options, as well as project-scoped
configuration (that does not belong into the project file). The configuration is
loaded in the following order:

1. Global configuration folder (e.g. `~/.config/pixi/config.toml` on Linux,
   dependent on XDG_CONFIG_HOME)
2. Global .pixi folder: `~/.pixi/config.toml` (or `$PIXI_HOME/config.toml` if
   the `PIXI_HOME` environment variable is set)
3. Project-local .pixi folder: `$PIXI_PROJECT/.pixi/config.toml`
4. Command line arguments (`--tls-no-verify`, `--change-ps1=false` etc.)

!!! note
    To find the locations where `pixi` looks for configuration files, run
    `pixi` with `-v` or `--verbose`.

## Reference

The following reference describes all available configuration options.

```toml
# The default channels to select when running `pixi init` or `pixi global install`.
# This defaults to only conda-forge.
default_channels = ["conda-forge"]

# When set to false, the `(pixi)` prefix in the shell prompt is removed.
# This applies to the `pixi shell` subcommand.
# You can override this from the CLI with `--change-ps1`.
change_ps1 = true

# When set to true, the TLS certificates are not verified. Note that this is a
# security risk and should only be used for testing purposes or internal networks.
# You can override this from the CLI with `--tls-no-verify`.
tls_no_verify = false

# Override from where the authentication information is loaded.
# Usually we try to use the keyring to load authentication data from, and only use a JSON
# file as fallback. This option allows you to force the use of a JSON file.
# Read more in the authentication section.
authentication_override_file = "/path/to/your/override.json"

# configuration for conda channel-mirrors
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

```toml
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

```toml
[mirrors]
"https://conda.anaconda.org/conda-forge" = [
    "oci://ghcr.io/channel-mirrors/conda-forge"
]
```

The GHCR mirror also contains `bioconda` packages. You can search the [available
packages on Github](https://github.com/orgs/channel-mirrors/packages).
