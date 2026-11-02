# Channel-provided virtual package detectors

This example publishes two noarch Python packages to a local channel:

- `username-length-detect` reports `__username_length`, the number of characters in your username.
- `long-username` requires `__username_length >=10`.

It is based on [Julian Hofer's virtual package detector demo](https://github.com/Hofer-Julian/virtual-package-detector-demo).
The complete source is in [`docs/source_files/pixi_workspaces/virtual_package_detector`](https://github.com/prefix-dev/pixi/tree/main/docs/source_files/pixi_workspaces/virtual_package_detector).

## Prepare the example

Use a development build of [Pixi](../index.md) with support for detector dependencies from workspace channels, and [Nushell](https://www.nushell.sh/).
The commands below use Nushell. From a checkout of the Pixi repository, enter the example directory:

```nu
cd docs/source_files/pixi_workspaces/virtual_package_detector
```

Run the remaining commands from this directory.

## The detector

The detector prints a JSON report to standard output:

```python title="detector/src/username_length_detect/__init__.py"
--8<-- "docs/source_files/pixi_workspaces/virtual_package_detector/detector/src/username_length_detect/__init__.py"
```

The outer `version: 1` selects the detector protocol version.
`virtual_packages` maps names starting with `__` to their detected values.
Here, `__username_length` has the username's character count as its version and `"0"` as its `build_string`.
`cache.watch_env` invalidates the cached report when any username variable changes.

The executable name must match the detector package name, `username-length-detect`.
The `project.scripts` entry creates that executable:

```toml title="detector/pyproject.toml"
--8<-- "docs/source_files/pixi_workspaces/virtual_package_detector/detector/pyproject.toml"
```

```toml title="detector/pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/virtual_package_detector/detector/pixi.toml"
```

`pixi-build-python` builds this pure Python package as `noarch`, so the same artifact works on all five consumer platforms.

## The dependent package

The application only prints its name:

```python title="package/src/long_username/__init__.py"
--8<-- "docs/source_files/pixi_workspaces/virtual_package_detector/package/src/long_username/__init__.py"
```

```toml title="package/pyproject.toml"
--8<-- "docs/source_files/pixi_workspaces/virtual_package_detector/package/pyproject.toml"
```

Its conda runtime dependency, not the Python code, requires a username of at least ten characters:

```toml title="package/pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/virtual_package_detector/package/pixi.toml"
```

This package is also built as `noarch`.

## Publish and register the detector

Build both packages into the local channel:

```nu
pixi publish --path detector --target-channel channel
pixi publish --path package --target-channel channel
```

The index configuration registers the detector and the virtual package it provides:

```toml title="index-config.toml"
--8<-- "docs/source_files/pixi_workspaces/virtual_package_detector/index-config.toml"
```

Install `rattler-index` and reindex the channel with that configuration:

```nu
pixi global install "rattler-index>=0.33.1"
rattler-index --force --config index-config.toml fs channel
```

## Install the consumer

The consumer declares `__username_length = "10"` for `linux-64`, `linux-aarch64`, `osx-64`, `osx-arm64`, and `win-64`:

```toml title="consumer/pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/virtual_package_detector/consumer/pixi.toml"
```

The local channel supplies both example packages; conda-forge supplies Python and its dependencies.
Pixi installs the detector into a separate environment using these channels, with the detector package pinned to its registering channel.

The manifest's virtual package values describe the target platforms when solving the lock file.
They do not claim that your machine has those capabilities.
During installation, Pixi runs the trusted detector and checks its report against the declared requirements.

```nu
pixi install --manifest-path consumer
```

When Pixi asks whether to trust detectors from your channel, choose **Trust** and save the decision for this workspace/repository, not globally.
This allows code from the channel to run on your machine, including future detectors registered by that channel.
If your username has fewer than ten characters, the host check rejects the installation.

Once installation succeeds, run the application:

```nu
pixi run --manifest-path consumer start
```

It prints `long-username`.

## Check both outcomes with overrides

Use a command-scoped override to exercise the success case and create the lock file even if your actual username is shorter:

```nu
with-env {CONDA_OVERRIDE_USERNAME_LENGTH: "12"} {
    pixi install --manifest-path consumer
    pixi run --manifest-path consumer start
}
```

With the lock file in place, a value of `9` fails the host check:

```nu
with-env {CONDA_OVERRIDE_USERNAME_LENGTH: "9"} {
    pixi install --locked --manifest-path consumer
}
```

Use `pixi install` for the rejection check. `pixi run` may reuse an already installed environment rather than repeat full host detection.
These overrides apply only within their `with-env` blocks; they do not change the manifest or your username.
