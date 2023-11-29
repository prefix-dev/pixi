---
part: pixi
title: Getting Started
description: Package management made easy
---

Pixi is a package management tool for developers.
It allows the developer to install libraries and applications in a reproducible way.
Use pixi cross-platform, on Windows, Mac and Linux.

## Installation

To install `pixi` you can run the following command in your terminal:

=== "Linux & macOS"
    ```shell
    curl -fsSL https://pixi.sh/install.sh | bash
    ```

    The above invocation will automatically download the latest version of `pixi`, extract it, and move the `pixi` binary to `~/.pixi/bin`.
    If this directory does not already exist, the script will create it.

    The script will also update your `~/.bash_profile` to include `~/.pixi/bin` in your PATH, allowing you to invoke the `pixi` command from anywhere.

=== "Windows"
    PowerShell:
    ```powershell
    iwr -useb https://pixi.sh/install.ps1 | iex
    ```

    The above invocation will automatically download the latest version of `pixi`, extract it, and move the `pixi` binary to `LocalAppData/pixi/bin`.
    If this directory does not already exist, the script will create it.

    The command will also automatically add `LocalAppData/pixi/bin` to your path allowing you to invoke `pixi` from anywhere.


!!! tip

    You might need to restart your terminal or source your shell for the changes to take effect.


## Autocompletion

To get autocompletion run:

=== "Linux & macOS"
    ```shell
    # Pick your shell (use `echo $SHELL` to find the shell you are using.):
    echo 'eval "$(pixi completion --shell bash)"' >> ~/.bashrc
    echo 'eval "$(pixi completion --shell zsh)"' >> ~/.zshrc
    echo 'pixi completion --shell fish | source' >> ~/.config/fish/config.fish
    echo 'eval (pixi completion --shell elvish | slurp)' >> ~/.elvish/rc.elv
    ```
=== "Windows"
    PowerShell:
    ```powershell
    Add-Content -Path $PROFILE -Value '(& pixi completion --shell powershell) | Out-String | Invoke-Expression'
    ```


And then restart the shell or source the shell config file.

## Alternative installation methods

Although we recommend installing pixi through the above method we also provide additional installation methods.

### Homebrew

Pixi is available via homebrew. To install pixi via homebrew simply run:

```shell
brew install pixi
```

### Windows installer

We provide an `msi` installer on [our Github releases page](https://github.com/prefix-dev/pixi/releases/latest).
The installer will download pixi and add it to the path.

### Install from source

pixi is 100% written in Rust, and therefore it can be installed, built and tested with cargo.
To start using pixi from a source build run:

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

## Update
Updating is as simple as installing, rerunning the installation script gets you the latest version.

=== "Linux & macOS"
    ```shell
    curl -fsSL https://pixi.sh/install.sh | bash
    ```
    Or get a specific pixi version using:
    ```shell
    export PIXI_VERSION=vX.Y.Z && curl -fsSL https://pixi.sh/install.sh | bash
    ```
=== "Windows"
    PowerShell:
    ```powershell
    iwr -useb https://pixi.sh/install.ps1 | iex
    ```
    Or get a specific pixi version using:
    PowerShell:
    ```powershell
    $Env:PIXI_VERSION=vX.Y.Z && iwr -useb https://pixi.sh/install.ps1 | iex
    ```
!!! note
    If you used a package manager like `brew`, `mamba`, `conda`, `paru` to install `pixi`.
    Then use their builtin update mechanism. e.g. `brew update && brew upgrade pixi`

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
