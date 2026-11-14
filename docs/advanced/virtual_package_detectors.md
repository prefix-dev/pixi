# Channel-provided virtual package detectors

Virtual package detectors are external programs that detect host properties and report them as virtual packages.
This tutorial is for package and channel authors who want to make packages depend on those properties.
It demonstrates the full life cycle of a virtual package detector:

- creating a detector,
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
The JSON is built-up like so:

- `version` of the schema (at the time of writing, only `1` exists)
- `virtual_packages`, reporting the `version` and optional `build_string` of the virtual package `__username_length`
- `cache`, optionally describing when the detector result is invalidated. If this is not set, the result is cached for 1 hour

The Cargo manifest names the binary and declares its `serde_json` dependency:

```toml title="detector/Cargo.toml"
--8<-- "docs/source_files/pixi_workspaces/virtual_package_detector/detector/Cargo.toml"
```

The Pixi manifest configures how the detector is built:

!!! warning "Platform"
    The `pixi.toml` manifests below use `linux-64`.
    On another platform, replace `linux-64` with your native platform, such as `linux-aarch64`, `osx-arm64`, or `win-64`.

!!! warning "Preview feature"
    This tutorial uses `pixi-build`, which is behind a preview flag and will change until it is stabilized.

```toml title="detector/pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/virtual_package_detector/detector/pixi.toml"
```

`pixi-build-rust` reads the package name and version from `Cargo.toml`.
You don't have to worry about the complexity of the rest of the package manifest for now.

??? note "I want to worry"

    The `RUSTFLAGS` settings enable static linking so the detector doesn't need shared-library run dependencies.
    Normally, a hosted channel can declare a [channel relation](https://github.com/conda/ceps/blob/main/cep-0042.md#channel-references) to another channel providing those run dependencies.
    Channel references must be relative paths, so the local file channel used in this tutorial cannot reference hosted conda-forge this way.

## The dependent package

In most cases, a virtual package is a run dependency of the dependent package.
That way you can ensure that said dependent package can only be installed if the host machine is suitable.
In our case, we define a package that can only be installed if the detected username length is `>=3`.

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

With `pixi publish` we build both the detector and the dependent package and add them to a local channel at `./channel`.

```bash
pixi publish --path detector --target-channel ./channel
pixi publish --path package --target-channel ./channel
```

Next, we create a file that registers the detector at the channel.

```toml title="index-config.toml"
--8<-- "docs/source_files/pixi_workspaces/virtual_package_detector/index-config.toml"
```

Finally, install `rattler-index` and use it to register the detector:

```bash
pixi global install "rattler-index>=0.33.1"
rattler-index --force --config index-config.toml fs channel
```

This step is there to allow us to run things fully locally.
In most cases, you will upload your detector to a hosted channel like [prefix.dev](https://prefix.dev/docs/prefix/virtual_package_detectors) which is much simpler.

## Install the consumer

Now we switch to the role of the author of a consumer workspace which wants to use the `long-username` package.
The workspace author declares the supported platforms so Pixi can lock all environments and their dependencies for those platforms, regardless of the machine generating the lock file.
When a consumer runs the workspace, Pixi checks whether the host satisfies one of those platforms.

Here, the workspace author supports `linux-64` machines with `__username_length` of at least `3`.

```toml title="consumer/pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/virtual_package_detector/consumer/pixi.toml"
```

In `workspace.platforms`, `__username_length = "3"` declares version `3` for the solver.
The host check accepts a detected version of `3` or newer, so this platform supports usernames with at least three characters.
This is a version declaration, not a dependency constraint like `__username_length = ">=3"` in `package.run-dependencies`.
See [Declaring virtual packages per platform](../workspace/multi_platform_configuration.md#declaring-virtual-packages-per-platform) for the platform syntax.

The local channel supplies `long-username` and the detector, while conda-forge supplies the application's runtime dependencies.

Let's run the task `start` which simply runs the executable `long-username`.

```bash
pixi run --manifest-path consumer start
```

First, Pixi checks which platform is suitable for your system.
We only defined one platform, so Pixi only has to check that one.
Then it notices that `__username_length` is provided by a virtual package detector.

In an interactive terminal, Pixi asks whether to trust detectors from the channel.
The absolute channel path depends on where the example is saved:

```text
Detectors inspect your system to identify capabilities that packages depend on, and run code on your machine.
Trust applies to current and future detectors from this channel.
? Trust detectors from file:///path/to/example/channel/? ›
❯ Don't trust
  Trust
```

Virtual package detectors can run arbitrary code, so only choose **Trust** if the channel is trusted.
Pixi then asks where to save the decision; **Don't save** keeps it limited to the current invocation.
After consent, the detector `username-length-detect` runs.
If it reports that your username has fewer than three characters, the platform isn't selected.
Since that is the only platform we defined, Pixi will then report that your host isn't supported.

If the username was long enough, Pixi selects the platform, installs the environment and runs the executable:

```text
✨ Pixi task (start): long-username
long-username
```

The same mechanism lets packages require arbitrary host properties reported by a detector, without those properties being built into Pixi.