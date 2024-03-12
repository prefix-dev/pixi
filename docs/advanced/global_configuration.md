# Global configuration in pixi

Pixi supports some global configuration options, as well as project-scoped configuration (that does not belong into the project file).
The configuration is loaded in the following order:

1. Global configuration folder (e.g. `~/.config/pixi/config.toml` on Linux, dependent on XDG_CONFIG_HOME)
2. Global .pixi folder: `~/.pixi/config.toml` (or `$PIXI_HOME/config.toml` if the `PIXI_HOME` environment variable is set)
3. Project-local .pixi folder: `$PIXI_PROJECT/.pixi/config.toml`
4. Command line arguments (`--tls-no-verify`, `--change-ps1=false` etc.)

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
```
