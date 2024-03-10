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
default_channels = ["bioconda", "conda-forge"]
default_mirrors = ["https://my.custom.mirror/{channel}"]

[[channels]]
name = "conda-forge"
mirrors = [
    "https://conda.anaconda.org/conda-forge",
    "https://repo.prefix.dev/conda-forge"
]

[[channels]]
name = "prefix-internal"
url = "https://repo.prefix.dev/prefix-internal"

[[channels]]
name = "bioconda"
mirrors = [
    "https://repo.prefix.dev/bioconda"
]

[[channels]]
name = "my-private-channel"
url = "https://my.private.quetz/get/my-private-channel"
mirrors = [
    "https://repo.artifactory.com/conda-mirror-my-private-channel"
]
```

## Use cases

### Use the same mirror everywhere

If you want to use the same mirror for all channels, you can set the `default_mirrors` key in the configuration file.

```toml title="rattler.toml"
default_mirrors = ["https://my.custom.mirror/{channel}"]

[[channels]]
name = "bioconda"
mirrors = ["https://repo.prefix.dev/bioconda"]
```

This results in all channels being redirected to this mirror.

```toml title="pixi.toml"
...
channels = [
    "conda-forge",
    "bioconda",
    "https://conda.anaconda.org/cf-staging",
    "https://repo.prefix.dev/prefix-internal"
]
...
```

- `conda-forge` will be redirected to `https://my.custom.mirror/conda-forge`
- `bioconda` will be redirected to `https://repo.prefix.dev/bioconda` since it has higher priority than `default_mirrors`
- `https://conda.anaconda.org/cf-staging` and `https://repo.prefix.dev/prefix-internal` will not being redirected since they specify a full URL

!!!tip "Use `prefix.dev` mirrors"
    If you want to use the mirrors hosted on [prefix.dev](https://prefix.dev), you can use the following configuration:

    ```toml title="rattler.toml"
    default_mirrors = ["https://repo.prefix.dev/{channel}"]
    ```

### Default channels

```toml title="rattler.toml"
default_channels = ["my-private-channel", "bioconda", "conda-forge"]

[[channels]]
name = "my-private-channel"
url = "https://my.private.quetz/get/my-private-channel"
```

This will result in the channels `bioconda` and `conda-forge` and `my-private-channel` being used when you run `pixi init` or `pixi global install`.
These channels will also be used by `rattler-build` if you don't specify other channels with `-c`.

### Multiple mirrors

```toml title="rattler.toml"
[[channels]]
name = "conda-forge"
mirrors = [
    "https://conda.anaconda.org/conda-forge",
    {url = "https://repo.artifactory.com/conda-mirror-conda-forge", use_zstd = false}
    "oci://ghcr.io/conda-channel-mirrors/conda-forge"
]
```

This will result in `conda-forge` being redirected to all mirrors specified in `mirrors`.
`pixi` and `rattler-build` will use the fastest (TODO: what is the real behavior?) mirror available.

### `use_zstd`, `use_bz2` and `use_jlap`

You can specify whether `pixi` and `rattler-build` should use the `repodata.json.zst`, etc. if available by setting `use_zstd`, `use_bz2` and `use_jlap`.
This is needed for some proxies like older versions of artifactory ([RTFACT-29886](https://jfrog.atlassian.net/jira/software/c/projects/RTFACT/issues/RTFACT-29886)).

```toml title="rattler.toml"
default_mirrors = [
    {"https://my.custom.mirror/{channel}", use_bz2 = false, use_zstd = false}
]

[[channels]]
name = "conda-forge"
mirrors = [
    {url = "https://repo.artifactory.com/conda-mirror-conda-forge", use_zstd = false}
]

[[channels]]
name = "bioconda"
use_zstd = false
mirrors = [
    "https://repo.prefix.dev/bioconda"
]
```

### Use private channels

```toml title="rattler.toml"
default_channels = ["conda-forge", "private-channel"]

[[channels]]
name = "private-channel"
url = "https://my.private.quetz/get/my-private-channel"
```

This will result in `private-channel` being redirected to `https://my.private.quetz/get/my-private-channel`.
If you want to use a mirror for this private channel, you can additionally specify `mirrors` for this channel.

```toml title="rattler.toml"
default_channels = ["conda-forge", "private-channel"]

[[channels]]
name = "private-channel"
url = "https://my.private.quetz/get/my-private-channel"
mirrors = [
    "https://repo.artifactory.com/conda-mirror-my-private-channel"
]
```

This will result in `private-channel` being redirected to `https://repo.artifactory.com/conda-mirror-my-private-channel`, but `https://my.private.quetz/get/my-private-channel` is still used in the `pixi.lock` file. Thus, the canonical URL is still `https://my.private.quetz/get/my-private-channel`.

### OCI mirrors

You can also use [OCI](https://opencontainers.org/) mirrors.

```toml title="rattler.toml"
[[channels]]
name = "conda-forge"
mirrors = [
    "oci://ghcr.io/conda-channel-mirrors/conda-forge"
]
```
