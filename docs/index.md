---
part: pixi
title: Getting Started
description: Package management made easy
---
# Getting Started
![Pixi with magic wand](assets/pixi.webp)

Pixi is a package management tool for developers.
It allows the developer to install libraries and applications in a reproducible way.
Use pixi cross-platform, on Windows, Mac and Linux.

## Installation

To install `pixi` you can run the following command in your terminal:

=== "Linux & macOS"
    ```bash
    curl -fsSL https://pixi.sh/install.sh | bash
    ```

    The above invocation will automatically download the latest version of `pixi`, extract it, and move the `pixi` binary to `~/.pixi/bin`.
    If this directory does not already exist, the script will create it.

    The script will also update your `~/.bash_profile` to include `~/.pixi/bin` in your PATH, allowing you to invoke the `pixi` command from anywhere.

=== "Windows"
    `PowerShell`:
    ```powershell
    iwr -useb https://pixi.sh/install.ps1 | iex
    ```
    `winget`:
    ```
    winget install prefix-dev.pixi
    ```
    The above invocation will automatically download the latest version of `pixi`, extract it, and move the `pixi` binary to `LocalAppData/pixi/bin`.
    If this directory does not already exist, the script will create it.

    The command will also automatically add `LocalAppData/pixi/bin` to your path allowing you to invoke `pixi` from anywhere.


!!! tip

    You might need to restart your terminal or source your shell for the changes to take effect.

You can find more options for the installation script [here](#installer-script-options).

## Autocompletion

To get autocompletion follow the instructions for your shell.
Afterwards, restart the shell or source the shell config file.


### Bash (default on most Linux systems)

```bash
echo 'eval "$(pixi completion --shell bash)"' >> ~/.bashrc
```
### Zsh (default on macOS)

```zsh
echo 'eval "$(pixi completion --shell zsh)"' >> ~/.zshrc
```

### PowerShell (pre-installed on all Windows systems)

```pwsh
Add-Content -Path $PROFILE -Value '(& pixi completion --shell powershell) | Out-String | Invoke-Expression'
```

!!! tip "Failure because no profile file exists"
    Make sure your profile file exists, otherwise create it with:
    ```PowerShell
    New-Item -Path $PROFILE -ItemType File -Force
    ```


### Fish

```fish
echo 'pixi completion --shell fish | source' > ~/.config/fish/completions/pixi.fish
```

### Nushell

Add the following to the end of your Nushell env file (find it by running `$nu.env-path` in Nushell):

```nushell
mkdir ~/.cache/pixi
pixi completion --shell nushell | save -f ~/.cache/pixi/completions.nu
```

And add the following to the end of your Nushell configuration (find it by running `$nu.config-path`):

```nushell
use ~/.cache/pixi/completions.nu *
```

### Elvish

```elv
echo 'eval (pixi completion --shell elvish | slurp)' >> ~/.elvish/rc.elv
```

## Alternative installation methods

Although we recommend installing pixi through the above method we also provide additional installation methods.

### Homebrew

Pixi is available via homebrew. To install pixi via homebrew simply run:

```shell
brew install pixi
```

### Windows installer

We provide an `msi` installer on [our GitHub releases page](https://github.com/prefix-dev/pixi/releases/latest).
The installer will download pixi and add it to the path.

### Install from source

pixi is 100% written in Rust, and therefore it can be installed, built and tested with cargo.
To start using pixi from a source build run:

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
its [compile steps](https://github.com/mamba-org/rattler/tree/main#give-it-a-try).

### Installer script options

=== "Linux & macOS"

    The installation script has several options that can be manipulated through environment variables.

    | Variable             | Description                                                                        | Default Value         |
    |----------------------|------------------------------------------------------------------------------------|-----------------------|
    | `PIXI_VERSION`       | The version of pixi getting installed, can be used to up- or down-grade.           | `latest`              |
    | `PIXI_HOME`          | The location of the binary folder.                                                 | `$HOME/.pixi`         |
    | `PIXI_ARCH`          | The architecture the pixi version was built for.                                   | `uname -m`            |
    | `PIXI_NO_PATH_UPDATE`| If set the `$PATH` will not be updated to add `pixi` to it.                        |                       |
    | `TMP_DIR`            | The temporary directory the script uses to download to and unpack the binary from. | `/tmp`                |

    For example, on Apple Silicon, you can force the installation of the x86 version:
    ```shell
    curl -fsSL https://pixi.sh/install.sh | PIXI_ARCH=x86_64 bash
    ```
    Or set the version
    ```shell
    curl -fsSL https://pixi.sh/install.sh | PIXI_VERSION=v0.18.0 bash
    ```

=== "Windows"

    The installation script has several options that can be manipulated through environment variables.

    | Variable         | Environment variable | Description                                                                       | Default Value               |
    |------------------|----------------------|-----------------------------------------------------------------------------------|-----------------------------|
    | `PixiVersion`    | `PIXI_VERSION`       |The version of pixi getting installed, can be used to up- or down-grade.           | `latest`                    |
    | `PixiHome`       | `PIXI_HOME`          | The location of the installation.                                                 | `$Env:USERPROFILE\.pixi`    |
    | `NoPathUpdate`   |                      | If set, the `$PATH` will not be updated to add `pixi` to it.                      |                             |

    For example, set the version using:

    ```powershell
    iwr -useb https://pixi.sh/install.ps1 | iex -Args "-PixiVersion v0.18.0"
    ```
## Update

Updating is as simple as installing, rerunning the installation script gets you the latest version.

```shell
pixi self-update
```
Or get a specific pixi version using:
```shell
pixi self-update --version x.y.z
```

!!! note
    If you've used a package manager like `brew`, `mamba`, `conda`, `paru` etc. to install `pixi`
    you must use the built-in update mechanism. e.g. `brew upgrade pixi`.

## Uninstall

To uninstall pixi from your system, simply remove the binary.

=== "Linux & macOS"
    ```shell
    rm ~/.pixi/bin/pixi
    ```
=== "Windows"
    ```shell
    $PIXI_BIN = "$Env:LocalAppData\pixi\bin\pixi"; Remove-Item -Path $PIXI_BIN
    ```

After this command, you can still use the tools you installed with pixi.
To remove these as well, just remove the whole `~/.pixi` directory and remove the directory from your path.
