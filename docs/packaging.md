---
part: pixi
title: Packaging pixi
description: How to package pixi for distribution with another package manager?
---
This is a guide for distribution maintainers wanting to package pixi for a different package manager.
Users of pixi can ignore this page.

## Building

Pixi is written in Rust and compiled using Cargo, which are needed as compile-time dependencies.
At runtime pixi needs no dependencies in other than the runtime it was compiled against (`libc`, ...).

To build pixi run
```shell
cargo build --locked --profile dist
```
Instead of using the predefined `dist` profile, which is optimized for binary size, you can also pass other options to
let cargo optimize the binary for other metrics.

### Build-time Options

Pixi provides some compile-time options, which can influence the build

#### TLS

By default, pixi is built with Rustls TLS implementation. You can compile pixi using the platform native TLS implementation
using by adding `--no-default-features --feature native-tls` to the build command. Note that this might add additional
runtime dependencies, such as OpenSSL on Linux.

#### Self-Update

Pixi has a self-update functionality. When pixi is installed using another package manager one usually doesn't want pixi
to try to update itself and instead let it be updated by the package manager.
For this reason the self-update feature is disabled by default. It can be enabled by adding `--feature self_update` to
the build command.

When the self-update feature is disabled and a user tries to run `pixi self-update` an error message is displayed. This
message can be customized by setting the `PIXI_SELF_UPDATE_DISABLED_MESSAGE` environment variable at build time to point
the user to the package manager they should be using to update pixi.
```shell
PIXI_SELF_UPDATE_DISABLED_MESSAGE="`self-update` has been disabled for this build. Run `brew upgrade pixi` instead" cargo build --locked --profile dist
```

#### Custom version

You can specify a custom version string to be used in the `--version` output by setting the `PIXI_VERSION` environment variable during the build.

```shell
PIXI_VERSION="HEAD-123456" cargo build --locked --profile dist
```

## Shell completion

After building pixi you can generate shell autocompletion scripts by running
```shell
pixi completion --shell <SHELL>
```
and saving the output to a file.
Currently supported shells are `bash`, `elvish`, `fish`, `nushell`, `powershell` and `zsh`.
