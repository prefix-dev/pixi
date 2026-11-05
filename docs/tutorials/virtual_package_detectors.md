# Channel-provided virtual package detectors

Virtual package detectors are external programs that detect hardware capabilities on your system and inject virtual packages based on those findings.
This example demonstrates the full life cycle of a virtual package detector:

- creating a detector,
- publishing it to a channel, and finally
- depending on it as a consumer.

The example itself is kind of silly.
The detector `username-length-detect` reports the virtual package `__username_length`, whose version describes the number of characters in your username.
A second package `long-username` then requires `__username_length >=10`.

## Prepare the example

Use a development build of Pixi with virtual package detector support.
From a checkout of the Pixi repository, enter the example directory:

```console
cd docs/source_files/pixi_workspaces/virtual_package_detector
```

Run the remaining commands from this directory.

The manifests use `linux-64`. On another platform, replace it in `detector/pixi.toml`, `package/pixi.toml`, and `consumer/pixi.toml` with your native platform, such as `linux-aarch64`, `osx-64`, `osx-arm64`, or `win-64`.
These packages contain native binaries, not `noarch` packages. Build and run the example on the selected platform.

## The detector

The detector uses `serde_json` to print its JSON report to standard output:

```rust title="detector/src/main.rs"
--8<-- "docs/source_files/pixi_workspaces/virtual_package_detector/detector/src/main.rs"
```

The outer `version: 1` selects the detector protocol version.
`virtual_packages` maps names starting with `__` to their detected values.
Here, `__username_length` has the username's character count as its version and `"0"` as its `build_string`.
`cache.watch_env` invalidates the cached report when any username variable changes.

The executable name must match the detector package name, `username-length-detect`.
The Cargo package name creates the matching binary:

```toml title="detector/Cargo.toml"
--8<-- "docs/source_files/pixi_workspaces/virtual_package_detector/detector/Cargo.toml"
```

```toml title="detector/pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/virtual_package_detector/detector/pixi.toml"
```

`pixi-build-rust` reads the package name and version from `Cargo.toml`.
The compiler list is restricted to Rust because the default C compiler adds runtime package dependencies through run exports.
The Linux and Windows settings statically link the C runtime so the detector does not need runtime packages from another channel.
The Linux relocation setting produces a static non-PIE executable.

These settings avoid runtime dependencies from conda-forge: [CEP 42 channel relations](https://github.com/conda/ceps/blob/main/cep-0042.md#channel-references) use relative URLs, so a local file channel cannot reference hosted conda-forge.
A channel hosted alongside conda-forge could declare a channel relation instead.

## The dependent package

The application only prints its name:

```rust title="package/src/main.rs"
--8<-- "docs/source_files/pixi_workspaces/virtual_package_detector/package/src/main.rs"
```

```toml title="package/Cargo.toml"
--8<-- "docs/source_files/pixi_workspaces/virtual_package_detector/package/Cargo.toml"
```

Its conda runtime dependency, not the Rust code, requires a username of at least ten characters:

```toml title="package/pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/virtual_package_detector/package/pixi.toml"
```

## Publish and register the detector

Build both packages into the local channel:

```console
pixi publish --path detector --target-channel channel
pixi publish --path package --target-channel channel
```

The index configuration registers the detector and the virtual package it provides:

```toml title="index-config.toml"
--8<-- "docs/source_files/pixi_workspaces/virtual_package_detector/index-config.toml"
```

Install `rattler-index` and reindex the channel with that configuration:

```console
pixi global install "rattler-index>=0.33.1"
rattler-index --force --config index-config.toml fs channel
```

## Install the consumer

The consumer declares `__username_length = "10"` for the selected platform:

```toml title="consumer/pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/virtual_package_detector/consumer/pixi.toml"
```

The local channel supplies both packages. Neither needs Python or other conda runtime packages.
Pixi installs the detector into a separate environment from its registering channel.

The manifest's virtual package values describe the target platforms when solving the lock file.
They do not claim that your machine has those capabilities.
During installation, Pixi runs the trusted detector and checks its report against the declared requirements.

```console
pixi install --manifest-path consumer
```

When Pixi asks whether to trust detectors from your channel, choose **Trust** and save the decision for this workspace/repository, not globally.
This allows code from the channel to run on your machine, including future detectors registered by that channel.
If your username has fewer than ten characters, the host check rejects the installation.

Once installation succeeds, run the application:

```console
pixi run --manifest-path consumer start
```

It prints `long-username`.

## Check both outcomes with overrides

Open a separate terminal session for these checks so the overrides do not affect your normal environment.
Set the `CONDA_OVERRIDE_USERNAME_LENGTH` environment variable to `12` in that session, then install and run:

```console
pixi install --manifest-path consumer
pixi run --manifest-path consumer start
```

With the lock file in place, change `CONDA_OVERRIDE_USERNAME_LENGTH` to `9`, then run:

```console
pixi install --locked --manifest-path consumer
```

Use `pixi install` for the rejection check. `pixi run` may reuse an already installed environment rather than repeat full host detection.
The overrides do not change the manifest or your username. Close the test terminal session when finished.
