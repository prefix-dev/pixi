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
[![Project Chat][chat-badge]][chat-url]
[![Pixi Badge][pixi-badge]][pixi-url]


[license-badge]: https://img.shields.io/badge/license-BSD--3--Clause-blue?style=flat-square
[chat-badge]: https://img.shields.io/discord/1082332781146800168.svg?label=&logo=discord&logoColor=ffffff&color=7389D8&labelColor=6A7EC2&style=flat-square
[chat-url]: https://discord.gg/kKV8ZxyzY4
[pixi-badge]:https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/prefix-dev/pixi/main/assets/badge/v0.json&style=flat-square
[pixi-url]: https://pixi.sh

</h1>

# Pixi: Package Management Made Easy

## Overview

`pixi` is a cross-platform, multi-language package manager and workflow tool built on the foundation of the conda ecosystem. It provides developers with an exceptional experience similar to popular package managers like `cargo` or `yarn`, but for any language.

Developed with ❤️ at [prefix.dev](https://prefix.dev).
[![Real-time pixi_demo](https://github.com/prefix-dev/pixi/assets/12893423/0fc8f8c8-ac13-4c14-891b-dc613f25475b)](https://asciinema.org/a/636482)

## Highlights

- Supports **multiple languages** including Python, C++, and R using Conda packages. You can find available packages on [prefix.dev](https://prefix.dev).
- Compatible with all major operating systems: Linux, Windows, macOS (including Apple Silicon).
- Always includes an up-to-date **lock file**.
- Provides a clean and simple Cargo-like **command-line interface**.
- Allows you to install tools **per-project** or **system-wide**.
- Entirely written in **Rust** and built on top of the **[rattler](https://github.com/conda/rattler)** library.

## Getting Started

- ⚡ [Installation](#installation)
- ⚙️ [Examples](/examples)
- 📚 [Documentation](https://pixi.sh/)
- 😍 [Contributing](#contributing)
- 🔨 [Built using Pixi](#built-using-pixi)
- 🚀 [GitHub Action](https://github.com/prefix-dev/setup-pixi)

## Status

Pixi is ready for production!
We are working hard to keep file-format changes compatible with the previous
versions so that you can rely on Pixi with peace of mind.

Some notable features we envision for upcoming releases are:

- **Build and publish** your project as a Conda package.
- Support for **dependencies from source**.
- More powerful "global installation" of packages towards a deterministic setup of global packages on multiple machines.

## Installation

`pixi` can be installed on macOS, Linux, and Windows. The provided scripts will automatically download the latest version of `pixi`, extract it, and move the `pixi` binary to `~/.pixi/bin`. If this directory does not exist, the script will create it.

### macOS and Linux

To install Pixi on macOS and Linux, open a terminal and run the following command:

```bash
curl -fsSL https://pixi.sh/install.sh | sh
# or with brew
brew install pixi
```

The script will also update your `~/.bashrc` to include `~/.pixi/bin` in your `PATH`, allowing you to invoke the `pixi` command from anywhere.
You might need to restart your terminal or source your shell for the changes to take effect.

Starting with macOS Catalina [zsh is the default login shell and interactive shell](https://support.apple.com/en-us/102360). Therefore, you might want to use `zsh` instead of `bash` in the install command:

```zsh
curl -fsSL https://pixi.sh/install.sh | zsh
```

The script will also update your `~/.zshrc` to include `~/.pixi/bin` in your `PATH`, allowing you to invoke the `pixi` command from anywhere.

### Windows

To install Pixi on Windows, open a PowerShell terminal (you may need to run it as an administrator) and run the following command:

```powershell
powershell -ExecutionPolicy ByPass -c "irm -useb https://pixi.sh/install.ps1 | iex"
```
Changing the [execution policy](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_execution_policies?view=powershell-7.4#powershell-execution-policies) allows running a script from the internet.
Check the script you would be running with:
```powershell
powershell -c "irm -useb https://pixi.sh/install.ps1 | more"
```

The script will inform you once the installation is successful and add the `~/.pixi/bin` directory to your `PATH`, which will allow you to run the `pixi` command from any location.
Or with `winget`

```shell
winget install prefix-dev.pixi
```

### Autocompletion

To get autocompletion follow the instructions for your shell.
Afterwards, restart the shell or source the shell config file.

#### Bash (default on most Linux systems)

Add the following to the end of `~/.bashrc`:

```bash
# ~/.bashrc

eval "$(pixi completion --shell bash)"
```
#### Zsh (default on macOS)

Add the following to the end of `~/.zshrc`:


```zsh
# ~/.zshrc

eval "$(pixi completion --shell zsh)"
```

#### PowerShell (pre-installed on all Windows systems)

Add the following to the end of `Microsoft.PowerShell_profile.ps1`.
You can check the location of this file by querying the `$PROFILE` variable in PowerShell.
Typically the path is `~\Documents\PowerShell\Microsoft.PowerShell_profile.ps1` or
`~/.config/powershell/Microsoft.PowerShell_profile.ps1` on -Nix.

```pwsh
(& pixi completion --shell powershell) | Out-String | Invoke-Expression
```

#### Fish

Add the following to the end of `~/.config/fish/config.fish`:

```fish
# ~/.config/fish/config.fish

pixi completion --shell fish | source
```

#### Nushell

Add the following to your Nushell config file (find it by running `$nu.config-path` in Nushell):

```nushell
mkdir $"($nu.data-dir)/vendor/autoload"
pixi completion --shell nushell | save --force $"($nu.data-dir)/vendor/autoload/pixi-completions.nu"
```

#### Elvish

Add the following to the end of `~/.elvish/rc.elv`:

```elv
# ~/.elvish/rc.elv

eval (pixi completion --shell elvish | slurp)
```

### Distro Packages

[![Packaging status](https://repology.org/badge/vertical-allrepos/pixi.svg)](https://repology.org/project/pixi/versions)

#### Arch Linux

You can install `pixi` from the [extra repository](https://archlinux.org/packages/extra/x86_64/pixi/) using [pacman](https://wiki.archlinux.org/title/Pacman):

```shell
pacman -S pixi
```

#### Alpine Linux

`pixi` is available for [Alpine Edge](https://pkgs.alpinelinux.org/packages?name=pixi&branch=edge). It can be installed via [apk](https://wiki.alpinelinux.org/wiki/Alpine_Package_Keeper) after enabling the [testing repository](https://wiki.alpinelinux.org/wiki/Repositories).

```shell
apk add pixi
```

## Build/install from source

`pixi` is 100% written in Rust and therefore it can be installed, built and tested with cargo.
To start using `pixi` from a source build run:

```shell
cargo install --locked --git https://github.com/prefix-dev/pixi.git pixi
```

We don't publish to `crates.io` anymore, so you need to install it from the repository.
The reason for this is that we depend on some unpublished crates which disallows us to publish to `crates.io`.

or when you want to make changes use:

```shell
cargo build
cargo test
```

If you have any issues building because of the dependency on `rattler` checkout
it's [compile steps](https://github.com/conda/rattler/tree/main#give-it-a-try)

## Uninstall

To uninstall, the Pixi binary should be removed.
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
Pixi [version 0.50.1] - Developer Workflow and Environment Management for Multi-Platform, Language-Agnostic Workspaces.

Pixi is a versatile developer workflow tool designed to streamline the management of your workspace's dependencies, tasks, and environments.
Built on top of the Conda ecosystem, Pixi offers seamless integration with the PyPI ecosystem.

Basic Usage:
    Initialize pixi for a workspace:
    $ pixi init
    $ pixi add python numpy pytest

    Run a task:
    $ pixi task add test 'pytest -s'
    $ pixi run test

Found a Bug or Have a Feature Request?
Open an issue at: https://github.com/prefix-dev/pixi/issues

Need Help?
Ask a question on the Prefix Discord server: https://discord.gg/kKV8ZxyzY4

For more information, see the documentation at: https://pixi.sh

Usage: pixi [OPTIONS] <COMMAND>

Commands:
  add          Adds dependencies to the workspace [aliases: a]
  auth         Login to prefix.dev or anaconda.org servers to access private channels
  build        Workspace configuration
  clean        Cleanup the environments
  completion   Generates a completion script for a shell
  config       Configuration management
  exec         Run a command and install it in a temporary environment [aliases: x]
  global       Subcommand for global package management actions [aliases: g]
  info         Information about the system, workspace and environments for the current machine
  init         Creates a new workspace
  import       Imports a file into an environment in an existing workspace.
  install      Install an environment, both updating the lockfile and installing the environment [aliases: i]
  list         List workspace's packages [aliases: ls]
  lock         Solve environment and update the lock file without installing the environments
  reinstall    Re-install an environment, both updating the lockfile and re-installing the environment
  remove       Removes dependencies from the workspace [aliases: rm]
  run          Runs task in the pixi environment [aliases: r]
  search       Search a conda package
  self-update  Update pixi to the latest version or a specific version
  shell        Start a shell in a pixi environment, run `exit` to leave the shell [aliases: s]
  shell-hook   Print the pixi environment activation script
  task         Interact with tasks in the workspace
  tree         Show a tree of workspace dependencies [aliases: t]
  update       The `update` command checks if there are newer versions of the dependencies and updates the `pixi.lock` file and environments accordingly
  upgrade      Checks if there are newer versions of the dependencies and upgrades them in the lockfile and manifest file
  upload       Upload a conda package
  workspace    Modify the workspace configuration file through the command line
  help         Print this message or the help of the given subcommand(s)

Options:
  -V, --version  Print version

Global Options:
  -h, --help           Display help information
  -v, --verbose...     Increase logging verbosity (-v for warnings, -vv for info, -vvv for debug, -vvvv for trace)
  -q, --quiet...       Decrease logging verbosity (quiet mode)
      --color <COLOR>  Whether the log needs to be colored [env: PIXI_COLOR=] [default: auto] [possible values: always, never, auto]
      --no-progress    Hide all progress bars, always turned on if stderr is not a terminal [env: PIXI_NO_PROGRESS=]
```

## Creating a Pixi project

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

You can use Pixi in GitHub Actions to install dependencies and run commands.
It supports automatic caching of your environments.

```yml
- uses: prefix-dev/setup-pixi@v0.8.1
- run: pixi exec cowpy "Thanks for using pixi"
```

See the [documentation](https://pixi.sh/latest/advanced/github_actions) for more details.

<a name="contributing"></a>

## Contributing 😍

We would absolutely love for you to contribute to Pixi!
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

<a name="pixibuilt"></a>

## Built using Pixi

To see what's being built with `pixi` check out the [Community](/docs/misc/Community.md) page.
