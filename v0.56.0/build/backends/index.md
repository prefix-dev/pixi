To decouple the building of a conda package from Pixi we provide something what are called build backends. These are essentially executables following a specific protocol that is implemented for both Pixi and the build backend. This also allows for decoupling of the build backend from Pixi and it's manifest specification.

The Prefix.dev managed backends are being developed in the [pixi-build-backends](https://github.com/prefix-dev/pixi-build-backends) repository, and have their own [documentation](https://prefix-dev.github.io/pixi-build-backends/).

### Installation

Install a certain build backend by adding it to the `package.build` section of the manifest file.:

```toml
[package.build.backend]
channels = [
  "https://prefix.dev/pixi-build-backends",
  "https://prefix.dev/conda-forge",
]
name = "pixi-build-python"
version = "0.1.*"

```

For custom backend channels, you can add the channel to the `channels` section of the manifest file:

```toml
[package.build]
backend = { name = "pixi-build-python", version = "==0.3.2" }
channels = [
  "https://prefix.dev/pixi-build-backends",
  "https://prefix.dev/conda-forge",
]

```

### Overriding the Build Backend

Sometimes you want to override the build backend that is used by pixi. Meaning overriding the backend that is specified in the [`[package.build]`](../../reference/pixi_manifest/#build-table). We currently have two environment variables that allow for this:

1. `PIXI_BUILD_BACKEND_OVERRIDE`: This environment variable allows for overriding of one or multiple backends. Use `{name}={path}` to specify a backend name mapped to a path and `,` to separate multiple backends. For example: `pixi-build-cmake=/path/to/bin,pixi-build-python` will:
   1. override the `pixi-build-cmake` backend with the executable located at `/path/to/bin`
   1. and will use the `pixi-build-python` backend from the `PATH`.
1. `PIXI_BUILD_BACKEND_OVERRIDE_ALL`: If this environment variable is set to *some* value e.g `1` or `true`, it will not install any backends in isolation and will assume that all backends are overridden and available in the `PATH`. This is useful for development purposes. e.g `PIXI_BUILD_BACKEND_OVERRIDE_ALL=1 pixi install`
