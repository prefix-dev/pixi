<h1>
  <a href="https://github.com/prefix-dev/pixi/">
    <img alt="banner" src="https://github.com/prefix-dev/pixi/assets/4995967/2f45b4a8-2976-4f06-bc88-9825c282df84">
  </a>
</h1>

<h1 align="center">

![License][license-badge]
[![Build Status][build-badge]][build]
[![Project Chat][chat-badge]][chat-url]

[license-badge]: https://img.shields.io/badge/license-BSD--3--Clause-blue?style=flat-square
[build-badge]: https://img.shields.io/github/actions/workflow/status/prefix-dev/pixi/rust.yml?style=flat-square&branch=main
[build]: https://github.com/prefix-dev/pixi/actions/
[chat-badge]: https://img.shields.io/discord/1082332781146800168.svg?label=&logo=discord&logoColor=ffffff&color=7389D8&labelColor=6A7EC2&style=flat-square
[chat-url]: https://discord.gg/kKV8ZxyzY4

</h1>

# pixi: Package management made easy

`pixi` is a cross-platform, multi-language package manager and workflow tool
build on the shoulders of the conda ecosystem.

`pixi` provides all developers the exceptional experience that is usually found
with package managers like `cargo` or `yarn` but for any language.

https://github.com/prefix-dev/pixi/assets/885054/64666dee-841d-4680-9a61-7927913bc4e2

## Highlights

- Support for **multiple languages** like Python, C++ and R using Conda packages
- **All OS's**: Linux, Windows, macOS (including Apple Silicon)
- A **lockfile** is always included and always up-to-date.
- A clean and simple Cargo-like **command-line interface**.
- Install tools **per-project** or **system-wide**.
- Completely written in **Rust** and build on top of the **[rattler](https://github.com/mamba-org/rattler)** library.

## Getting Started

* ⚡ [Installation](#installation)
* ⚙️ [Examples](../examples)
* 📚 [Documentation](./getting_started.md)
* 😍 [Contributing](#contributing)

# Status

This project is currently in _alpha stage_.
There are many features that we want to add.
The file formats are still in flux.
Expect breaking changes while we work towards a v1.0.

Some notable features that we have in the pipeline are:

* **Build and publish** your project as a Conda package.
* Support for **PyPi packages**.
* Support **dependencies from source**.
* Improve docs, examples and user experience

# Installation
You can install `pixi` as a binary from the releases.
`pixi` can be installed on macOS, Linux, and Windows.
The provided scripts will automatically download the latest version of `pixi`, extract it, and move the `pixi` binary to `~/.pixi/bin`.
If this directory does not already exist, the script will create it.

## macOS and Linux
To install Pixi on macOS and Linux, open a terminal and run the following command:
```bash
curl -fsSL https://raw.githubusercontent.com/prefix-dev/pixi/main/install/install.sh | bash
```
The script will also update your ~/.bash_profile to include ~/.pixi/bin in your PATH, allowing you to invoke the pixi command from anywhere.
You might need to restart your terminal or source your shell for the changes to take effect.

## Windows
To install Pixi on Windows, open a PowerShell terminal (you may need to run it as an administrator) and run the following command:

```powershell
iwr -useb https://raw.githubusercontent.com/prefix-dev/pixi/main/install/install.ps1 | iex
```
The script will inform you once the installation is successful and add the ~/.pixi/bin directory to your PATH, which will allow you to run the pixi command from any location.

## Install from source

`pixi` is 100% written in Rust and therefor it can be installed, build and
tested with cargo.
To start using `pixi` from a source build run:

```shell
cargo install --locked --git https://github.com/prefix-dev/pixi.git
```

or when you want to make changes use:

```shell
cargo build
cargo test
```

If you have any issues building because of the dependency on `rattler` checkout
it's [compile steps](https://github.com/mamba-org/rattler/tree/main#give-it-a-try)

## Uninstall
To uninstall the pixi binary should be removed.
Delete `pixi` from the `$PIXI_DIR` which is default to `~/.pixi/bin/pixi`

So on linux its:
```shell
rm ~/.pixi/bin/pixi
```
and on windows:
```shell
$PIXI_BIN = "$Env:LocalAppData\pixi\bin\pixi"; Remove-Item -Path $PIXI_BIN
```
After this command you can still use the tools you installed with `pixi`.
To remove these as well just remove the whole `~/.pixi` directory and remove the directory from your path.

### Autocompletion

To get autocompletion run:

```shell
# On unix (MacOS or Linux), pick your shell (use `echo $SHELL` to find the shell you are using.):
echo 'eval "$(pixi completion --shell bash)"' >> ~/.bashrc
echo 'eval "$(pixi completion --shell zsh)"' >> ~/.zshrc
echo 'pixi completion --shell fish | source' >> ~/.config/fish/config.fish
echo 'eval (pixi completion --shell elvish | slurp)' >> ~/.elvish/rc.elv

# On Windows:
Add-Content -Path $PROFILE -Value 'Invoke-Expression (&pixi completion --shell powershell)'
```

And then restart the shell or source the shell config file.

# Usage

The cli looks as follows:

```bash
➜ pixi
A package management and workflow tool

Usage: pixi [OPTIONS] <COMMAND>

Commands:
  completion  Generates a completion script for a shell
  init        Creates a new project
  add         Adds a dependency to the project
  run         Runs command in project
  shell       Start a shell in the pixi environment of the project
  global      Global is the main entry point for the part of pixi that executes on the global(system) level
  auth        Login to prefix.dev or anaconda.org servers to access private channels
  install     Install all dependencies
  help        Print this message or the help of the given subcommand(s)

Options:
  -v, --verbose...  More output per occurrence
  -q, --quiet...    Less output per occurrence
  -h, --help        Print help
  -V, --version     Print version

```

## Creating a pixi project

Initialize a new project and navigate to the project directory

```
pixi init myproject
cd myproject
```

Add the dependencies you want to use

```
pixi add cowpy
```

Run the installed package in its environment

```bash
pixi run cowpy "Thanks for using pixi"
```

Activate a shell in the environment
```shell
pixi shell
cowpy "Thanks for using pixi"
exit
```

For more information check [the documentation](getting_started.md#basics-of-the-configuration-file)

## Installing a conda package globally

You can also globally install conda packages into their own environment.
This behavior is similar to [`pipx`](https://github.com/pypa/pipx).

```bash
pixi global install cowpy
```

For more examples
check [the documentation](./cli.md)

<a name="contributing"></a>

## Contributing 😍

We would absolutely love for you to contribute to `pixi`!
Whether you want to start an issue, fix a bug you encountered, or suggest an
improvement, every contribution is greatly appreciated.

If you're just getting started with our project or stepping into the Rust
ecosystem for the first time, we've got your back!
We recommend beginning with issues labeled as `good first issue`.
These are carefully chosen tasks that provide a smooth entry point into
contributing.These issues are typically more straightforward and are a great way
to get familiar with the project.

Got questions or ideas, or just want to chat? Join our lively conversations on
Discord.
We're very active and would be happy to welcome you to our
community. [Join our discord server today!][chat-url]

## Built using pixi

To see whats being built with `pixi` check out the [Community](Community.md) page.
