<h1>
  <a href="https://github.com/prefix-dev/pixi/">
    <picture>
      <source srcset="https://github.com/prefix-dev/pixi/assets/4995967/a3f9ff01-c9fb-4893-83c0-2a3f924df63e" type="image/webp">
      <source srcset="https://github.com/prefix-dev/pixi/assets/4995967/e42739c4-4cd9-49bb-9d0a-45f8088494b5" type="image/png">
      <img src="https://github.com/prefix-dev/pixi/assets/4995967/e42739c4-4cd9-49bb-9d0a-45f8088494b5" alt="banner">
    </picture>
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

# pixi: Package Management Made Easy

## Overview

`pixi` is a cross-platform, multi-language package manager and workflow tool built on the foundation of the conda ecosystem. It provides developers with an exceptional experience similar to popular package managers like `cargo` or `yarn`, but for any language.

Developed with ❤️ at [prefix.dev](https://prefix.dev).

![Real-time pixi_demo](https://github.com/ruben-arts/pixi/assets/12893423/8b1a1273-a210-4be2-a664-32076c535428)

## Highlights

- Supports **multiple languages** including Python, C++, and R using Conda packages. You can find available packages on [prefix.dev](https://prefix.dev).
- Compatible with all major operating systems: Linux, Windows, macOS (including Apple Silicon).
- Always includes an up-to-date **lockfile**.
- Provides a clean and simple Cargo-like **command-line interface**.
- Allows you to install tools **per-project** or **system-wide**.
- Entirely written in **Rust** and built on top of the **[rattler](https://github.com/mamba-org/rattler)** library.

## Getting Started

- ⚡ [Installation](#installation)
- ⚙️ [Examples](/examples)
- 📚 [Documentation](https://prefix.dev/docs/pixi/overview)
- 🔨 [Built using Pixi](#built-using-pixi)
- 🚀 [GitHub Action](https://github.com/prefix-dev/setup-pixi)

## Status

This project is currently in the _alpha stage_. It's actively under development, and we're planning to add many more features. The file formats are still subject to change, and you should expect breaking changes as we work towards a v1.0.

Some notable features we have in the pipeline are:

- **Build and publish** your project as a Conda package.
- Support for **PyPi packages**.
- Support for **dependencies from source**.
- Improvements to documentation, examples, and user experience.

## Installation

`pixi` can be installed on macOS, Linux, and Windows. The provided scripts will automatically download the latest version of `pixi`, extract it, and move the `pixi` binary to `~/.pixi/bin`. If this directory does not exist, the script will create it.

### macOS and Linux

To install Pixi on macOS and Linux, open a terminal and run the following command:

```bash
curl -fsSL https://pixi.sh/install.sh | bash
# or with brew
brew install pixi
```
The script will also update your ~/.bash_profile to include ~/.pixi/bin in your PATH, allowing you to invoke the pixi command from anywhere.
You might need to restart your terminal or source your shell for the changes to take effect.

## Windows
To install Pixi on Windows, open a PowerShell terminal (you may need to run it as an administrator) and run the following command:

```powershell
iwr -useb https://pixi.sh/install.ps1 | iex
```
The script will inform you once the installation is successful and add the ~/.pixi/bin directory to your PATH, which will allow you to run the pixi command from any location.

### Autocompletion

To get autocompletion run:

```shell
# On unix (MacOS or Linux), pick your shell (use `echo $SHELL` to find the shell you are using.):
echo 'eval "$(pixi completion --shell bash)"' >> ~/.bashrc
echo 'eval "$(pixi completion --shell zsh)"' >> ~/.zshrc
echo 'pixi completion --shell fish | source' >> ~/.config/fish/config.fish
echo 'eval (pixi completion --shell elvish | slurp)' >> ~/.elvish/rc.elv
```

For PowerShell on Windows, run the following command and then restart the shell or source the shell config file:

```pwsh
Add-Content -Path $PROFILE -Value '(& pixi completion --shell powershell) | Out-String | Invoke-Expression'
```

And then restart the shell or source the shell config file.

## Install from source

`pixi` is 100% written in Rust and therefore it can be installed, built and tested with cargo.
To start using `pixi` from a source build run:

```shell
cargo install --locked pixi
# Or to use the the latest `main` branch
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

So on Linux its:
```shell
rm ~/.pixi/bin/pixi
```
and on Windows:
```shell
$PIXI_BIN = "$Env:LocalAppData\pixi\bin\pixi"; Remove-Item -Path $PIXI_BIN
```
After this command you can still use the tools you installed with `pixi`.
To remove these as well just remove the whole `~/.pixi` directory and remove the directory from your path.

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
  run         Runs task in project
  shell       Start a shell in the pixi environment of the project
  global      Global is the main entry point for the part of pixi that executes on the global(system) level
  auth        Login to prefix.dev or anaconda.org servers to access private channels
  install     Install all dependencies
  task        Command management in project
  info        Information about the system and project
  upload      Upload a package to a prefix.dev channel
  search      Search a package, output will list the latest version of package
  project
  help        Print this message or the help of the given subcommand(s)

Options:
  -v, --verbose...     More output per occurrence
  -q, --quiet...       Less output per occurrence
      --color <COLOR>  Whether the log needs to be colored [default: auto] [possible values: always, never, auto]
  -h, --help           Print help
  -V, --version        Print version

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

## Installing a conda package globally

You can also globally install conda packages into their own environment.
This behavior is similar to [`pipx`](https://github.com/pypa/pipx) or [`condax`](https://github.com/mariusvniekerk/condax).

```bash
pixi global install cowpy
```

## Use in GitHub Actions

You can use pixi in GitHub Actions to install dependencies and run commands.

```yml
- uses: prefix-dev/setup-pixi@v0.2.0
  with:
    cache: true
- run: pixi run cowpy "Thanks for using pixi"
```

<a name="pixibuilt"></a>
## Built using pixi

To see what's being built with `pixi` check out the [Community](/docs/Community.md) page.
