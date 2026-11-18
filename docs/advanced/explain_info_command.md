`pixi info` prints out useful information to debug a situation or to get an overview of your machine/workspace.
This information can also be retrieved in `json` format using the `--json` flag, which can be useful for programmatically reading it.

```title="Running pixi info in the pixi repo"
➜ pixi info
      Pixi version: 0.13.0
          Platform: linux-64
  Virtual packages: __unix=0=0
                  : __linux=6.5.12=0
                  : __glibc=2.36=0
                  : __cuda=12.3=0
                  : __archspec=1=x86_64
         Cache dir: /home/user/.cache/rattler/cache
      Auth storage: /home/user/.rattler/credentials.json

Workspace
------------
           Version: 0.13.0
     Manifest file: /home/user/development/pixi/pixi.toml
      Last updated: 25-01-2024 10:29:08

Environments
------------
default
          Features: default
          Channels: conda-forge
  Dependency count: 10
      Dependencies: pre-commit, rust, openssl, pkg-config, git, mkdocs, mkdocs-material, pillow, cairosvg, compilers
  Target platforms: linux-64, osx-arm64, win-64, osx-64
             Tasks: docs, test-all, test, build, lint, install, build-docs
```

## Global info

The first part of the info output is information that is always available and tells you what Pixi can read on your machine.

### Platform

This defines the platform you're currently on according to pixi.
If this is incorrect, please file an issue on the [Pixi repo](https://github.com/prefix-dev/pixi).

### Virtual packages

The [virtual packages](../conda_ecosystem.md#virtual-packages-describing-the-host-system) that Pixi detects on your machine.

### Cache dir

The directory where Pixi stores its cache.
Checkout the [cache documentation](../workspace/environment.md#caching-packages) for more information.

### Auth storage

Check the [authentication documentation](../deployment/authentication.md)

### TLS root certs

The root certificates Pixi trusts for HTTPS connections.
This is shown in both the `Global` and the `Workspace` section.
The `Global` section shows what `pixi global` uses, which always reads the default configuration locations: it ignores a workspace's `.pixi/config.toml`, `--config-file` and `--no-config`.
Both take the build's default, the [`tls-root-certs`](../reference/pixi_configuration.md#tls-root-certs) setting, the `--tls-root-certs` flag or `PIXI_TLS_ROOT_CERTS`, and the `SSL_CERT_FILE` and `SSL_CERT_DIR` environment variables into account.
For example, `webpki (default)` means the bundled Mozilla roots are used because nothing else is configured, and `SSL_CERT_FILE=/etc/ssl/ca.pem (overrides system)` means the certificates from that file replace the system store.
PyPI requests use the same certificates, unless [`pypi-config.allow-insecure-host`](../reference/pixi_configuration.md#pypi-config) is set: uv then builds its own HTTP client, which picks its root certificates separately and may not match this line.

### Cache size

[requires `--extended`]

The size of the previously mentioned "Cache dir" in Mebibytes.

## Workspace info

Everything below `Workspace` is info about the workspace you're currently in.
This info is only available if your path has a [manifest file](../reference/pixi_manifest.md).

### Manifest file

The path to the [manifest file](../reference/pixi_manifest.md) that describes the workspace.

### Last updated

The last time the lock file was updated, either manually or by Pixi itself.

## Environment info

The environment info defined per environment. If you don't have any environments defined, this will only show the `default` environment.

### Features

This lists which features are enabled in the environment.
For the default this is only `default`

### Solve group

The [solve group](../workspace/multi_environment.md) the environment belongs to, shown only when configured.
Environments in the same solve group use the same versions of shared dependencies.

### Channels

The list of channels used in this environment.

### Dependency count

The amount of dependencies defined that are defined for this environment (not the amount of installed dependencies).

### Dependencies

The list of dependencies defined for this environment.

### Target platforms

The platforms the workspace has defined (for this environment).

### Selected platform and selection source

The platform used to filter this diagnostic output, separate from the installed platform below.
`Selection source` reports whether it came from the installed prefix, a host match, lock fallback, or the host baseline for a dependency-free environment.
In JSON, `selected_platform_source` uses `installed`, `host_match`, `lock_fallback`, or `host_baseline`.

Info never solves or verifies lock freshness, and only reads the lock when direct selection fails.
The next installation can select differently; see [diagnostic selection](../workspace/multi_platform_configuration.md#diagnostic-selection).

### Resolved platform

The platform the environment was resolved for the last time it was installed, shown as the conda subdir plus the virtual packages it was resolved with.
Only present once the environment has been installed.

### Minimum platform

What the installed packages actually require: the same subdir with only the virtual-package requirements that some installed package depends on, shown as match specs the way the packages spell them (`__glibc >=2.17`).
This can be weaker than the resolved platform and tells you how portable the installed environment is.
Only present once the environment has been installed.
