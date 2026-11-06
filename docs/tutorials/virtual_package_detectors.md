# Channel-provided virtual package detectors

Virtual package detectors are external programs that detect hardware capabilities on your system and inject virtual packages based on those findings.
This example demonstrates the full life cycle of a virtual package detector:

- Creating a detector,
- publishing it to a channel, and finally
- depending on it as a consumer.

The example itself is kind of silly.
The detector `username-length-detect` reports the virtual package `__username_length`, whose version describes the number of characters in your username.
A second package `long-username` then requires `__username_length >=3`.

## Let's get started

We will build two Rust packages: `username-length-detect` in `detector/` and `long-username` in `package/`.
We will then use them from a separate Pixi workspace in `consumer/`.

The source files have the following layout:

```text
.
├── detector
│   ├── Cargo.toml
│   ├── pixi.toml
│   └── src
│       └── main.rs
├── package
│   ├── Cargo.toml
│   ├── pixi.toml
│   └── src
│       └── main.rs
├── consumer
│   └── pixi.toml
└── index-config.toml
```

## The detector

The detector is a simple Rust application.

```rust title="detector/src/main.rs"
--8<-- "docs/source_files/pixi_workspaces/virtual_package_detector/detector/src/main.rs"
```

First it determines the username by checking a couple of environment variables until it finds one that is set.
Then it prints a JSON document to standard output.
The JSON is built-up like that:

- `version` of the schema, at the time of this writing only `1` exists
- `virtual_packages` reports `version` and optional `build_string` of the virtual package `__username_length`
- `cache` optionally describes when the detector result is invalidated. If that is not set, the result is cached for 1 hour 

The Cargo manifest names the binary and declares its `serde_json` dependency:

```toml title="detector/Cargo.toml"
--8<-- "docs/source_files/pixi_workspaces/virtual_package_detector/detector/Cargo.toml"
```

The Pixi manifest configures how the detector is built:

!!! warning "Platform"
    The `pixi.toml` manifests below use `linux-64`.
    On another platform, replace `linux-64` with your native platform, such as `linux-aarch64`, `osx-arm64`, or `win-64`.

```toml title="detector/pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/virtual_package_detector/detector/pixi.toml"
```

`pixi-build-rust` reads the package name and version from `Cargo.toml`.
The reason we limit the compiler to "rust" and set the `RUSTFLAGS` is to make sure that the resulting executable has no dependency on any shared libraries.

In normal circumstances, you don't have to ensure this since you can simply upload your detector that has a [channel relation](https://github.com/conda/ceps/blob/main/cep-0042.md#channel-references) to for example `conda-forge`.
However, that doesn't work wit a local file channel that we are going to create later in this tutorial.

## The dependent package

In most cases, a virtual package is a run dependency of dependent package.
That way you can ensure that said dependent package can only be installed if the host machine is suitable.
So in our case, we have to define a package that can only be installed if the username is `>=3`.

The application code is boring, it only prints its name:

```rust title="package/src/main.rs"
--8<-- "docs/source_files/pixi_workspaces/virtual_package_detector/package/src/main.rs"
```

Also, the Cargo manifest bears no surprises.

```toml title="package/Cargo.toml"
--8<-- "docs/source_files/pixi_workspaces/virtual_package_detector/package/Cargo.toml"
```

This is the interesting part, we set `run-dependencies` of the package so that it requires usernames `>=3`.

```toml title="package/pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/virtual_package_detector/package/pixi.toml"
```

## Publish and register the detector

With `pixi publish` we build both detector and dependent package and add it to local channel at `./channel`.

```bash
pixi publish --path detector --target-channel ./channel
pixi publish --path package --target-channel ./channel
```

Next we create a file that registers the detector at the channel.
Most hosted providers will have a user interface for that, the same applies for [hosted channels of prefix.dev](https://prefix.dev/docs/prefix/virtual_package_detectors)

```toml title="index-config.toml"
--8<-- "docs/source_files/pixi_workspaces/virtual_package_detector/index-config.toml"
```

Next install `rattler-index` and use it to register the detector:

```bash
pixi global install "rattler-index>=0.33.1"
rattler-index --force --config index-config.toml fs channel
```

## Install the consumer

Now we jump into the role of the consumer.
Here, you simply want to use the `long-username` package. 
Pixi forces you to think about which platforms you want to support beforehand, that way all environments and their dependencies can be stored in the lock file for all platforms, no matter which one you currently use.

In our case we want to support machines with `linux-64` and `__username_length` of `3`.

```toml title="consumer/pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/virtual_package_detector/consumer/pixi.toml"
```

The local channel supplies `long-username` and the detector, while conda-forge supplies the application's runtime dependencies.

Let's run the task `start` which simply runs the executable `long-username`.

```bash
pixi run --manifest-path consumer start
```

First, Pixi checks which platform is suitable for your system.
We, only defined one platform, so that only has to check that one.
Then it notices that `__username_length` is provided by a virtual package detector.

Now Pixi asks whether to trust detectors from your channel, virtual package detectors can run arbitrary code, so make sure you trust the channel before you choose **Trust**.
Then the detector `username-length-detect` runs.
If it reports that your username has fewer than three characters, the platform isn't selected.
Since that is the only platform we defined, Pixi will then report that your host isn't supported. 

If your username was long enough though, it will select the platform, install the environment and run the executable.
Which then prints "long-username".