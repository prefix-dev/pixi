# Channel configuration file

In corporate environments, you often run into the situation where you have to configure custom mirrors for channels like `conda-forge`.

This can be done by creating a file in one of the following locations:

1. `$PWD/rattler.toml`
2. `$PWD/.rattler.toml`
3. `$XDG_CONFIG_HOME/rattler/config.toml` (see [dirs crate](https://crates.io/crates/dirs))
4. `$HOME/.config/rattler/config.toml`
5. `$HOME/rattler.toml`
6. `$HOME/.rattler.toml`

If you want to use a different configuration location, you can override it with the `RATTLER_CHANNEL_CONFIG` environment variable.

!!!warning "Multiple configuration files"
    The first configuration file found will be used. If you have multiple configuration files from the ones listed above, the first one found will be used.
    `RATTLER_CHANNEL_CONFIG` has the highest priority.

It has the following structure:

```toml
default_channels = ["bioconda", "conda-forge", "my-private-channel"]
default_server = "https://my.custom.conda.server"

[[channels]]
name = "prefix-internal"
server = "https://repo.prefix.dev"

[mirrors]
"https://conda.anaconda.org" = [
    "https://conda.anaconda.org",
    "https://repo.prefix.dev",
    "oci://ghcr.io/conda-channel-mirrors/conda-forge"
]
"https://conda.anaconda.org/conda-forge" = [ # (1)!
   "https://internal-conda-forge.com/"
]
"https://repo.prefix.dev/prefix-internal" = [
    "https://custom.artifactory.cloud/conda-mirror-prefix-internal"
]
```

1. Note that longer (more specific) urls are checked first if the prefix matches. All requests to `conda-forge` from anaconda would thus go to the mirror.

## Use cases

### Use the same mirror everywhere

If you want to use the same mirror for all channels, you can set the `default_server` key in the configuration file.

```toml title="rattler.toml"
default_server = "https://my.custom.mirror"
```

This results in all channels being redirected to this mirror.

```toml title="pixi.toml"
server = "https://my.custom.mirror"
```

### Default channels

```toml title="rattler.toml"
default_channels = ["my-private-channel", "bioconda", "conda-forge"]

[[channels]]
name = "my-private-channel"
server = "https://my.private.quetz/get"
```

This will result in the channels `bioconda` and `conda-forge` and `my-private-channel` being used when you run `pixi init` or `pixi global install` and `rattler-build` (assuming you don't override the channels using `-c`).

`pixi init` would create the following `pixi.toml`:

```toml title="pixi.toml"
# ...
channels = [
    "my-private-channel",
    "bioconda",
    "conda-forge"
]
# ...

[[channels]]
name = "my-private-channel"
server = "https://my.private.quetz/get"
```

`pixi init --ignore-rattler-config` would create the following `pixi.toml`:

```toml title="pixi.toml"
# ...
channels = ["conda-forge"]
# ...
```

### Mirrors

Instead of providing a single mirror, you can also provide a list of mirrors.

```toml title="rattler.toml"
[mirrors]
"https://conda.anaconda.org" = [
    "https://conda.anaconda.org", # (1)!
    "https://repo.prefix.dev",
    "https://repo.artifactory.com/conda-mirror-conda-forge",
    "oci://ghcr.io/conda-channel-mirrors/conda-forge"
]
```

1. Need to re-add this to also use anaconda.org.

This will result in `conda-forge` being redirected to all mirrors specified in `mirrors`.
`pixi` and `rattler-build` will use all mirrors to download the packages.

!!!tip "Use `prefix.dev` mirrors"
    If you want to use the mirrors hosted on [prefix.dev](https://prefix.dev) instead of [anaconda.org](https://conda.anaconda.org), you can use the following configuration:

    ```toml title="rattler.toml"
    [mirrors]
    "https://conda.anaconda.org" = [
        "https://repo.prefix.dev"
    ]
    ```

### `use_zstd`, `use_bz2` and `use_jlap`

You can specify whether `pixi` and `rattler-build` should use the `repodata.json.zst`, etc. if available by setting `use_zstd`, `use_bz2` and `use_jlap`.
This is needed for some proxies like older versions of artifactory ([RTFACT-29886](https://jfrog.atlassian.net/jira/software/c/projects/RTFACT/issues/RTFACT-29886)).

```toml title="rattler.toml"
use_zstd = false
use_jlap = false
use_bz2 = false
```

### Use private channels

You can specify private channels in your `pixi.toml` and route them to a different URL.

```toml title="pixi.toml"
channels = ["conda-forge", "bioconda", "private-channel"]

[[channels]] # (1)!
name = "bioconda"
server = "https://conda.anaconda.org"

[[channels]]
name = "private-channel"
server = "https://my.private.quetz/get"
```

1. This is the default behavior.

This will result in `private-channel` being redirected to `https://my.private.quetz/get/my-private-channel`.
If you want to use a mirror for this private channel, you can override the channel URL in `rattler.toml` since `rattler.toml` takes precedence over `pixi.toml`.

```toml title="rattler.toml"
[mirrors]
"https://my.private.quetz/get/my-private-channel" = [
    "https://repo.artifactory.com/conda-mirror-my-private-channel"
]
```

This will result in `private-channel` being redirected to `https://repo.artifactory.com/conda-mirror-my-private-channel`, but `https://my.private.quetz/get/my-private-channel` is still used in the `pixi.lock` file.
Thus, the canonical URL is still `https://my.private.quetz/get/my-private-channel`.

### OCI mirrors

You can also use [OCI](https://opencontainers.org/) mirrors.

```toml title="rattler.toml"
[[channels]]
name = "conda-forge"
mirrors = [
    "oci://ghcr.io/conda-channel-mirrors/conda-forge"
]
```

```toml title="pixi.toml"
server = "oci://ghcr.io/conda-channel-mirrors"
channels = ["conda-forge", "bioconda"]
```
