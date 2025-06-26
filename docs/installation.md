## Installation
To install `pixi` you can run the following command in your terminal:

=== "Linux & macOS"
    ```bash
    curl -fsSL https://pixi.sh/install.sh | sh
    ```

    If your system doesn't have `curl`, you can use `wget`:

    ```bash
    wget -qO- https://pixi.sh/install.sh | sh
    ```

    ??? note "What does this do?"
        The above invocation will automatically download the latest version of `pixi`, extract it, and move the `pixi` binary to `~/.pixi/bin`.
        The script will also extend the `PATH` environment variable in the startup script of your shell to include `~/.pixi/bin`.
        This allows you to invoke `pixi` from anywhere.

=== "Windows"
    [Download installer](https://github.com/prefix-dev/pixi/releases/latest/download/pixi-x86_64-pc-windows-msvc.msi){ .md-button }

    Or run:

    ```powershell
    powershell -ExecutionPolicy ByPass -c "irm -useb https://pixi.sh/install.ps1 | iex"
    ```

    ??? note "What does this do?"
        The above invocation will automatically download the latest version of `pixi`, extract it, and move the `pixi` binary to `%UserProfile%\.pixi\bin`.
        The command will also add `%UserProfile%\.pixi\bin` to your `PATH` environment variable, allowing you to invoke `pixi` from anywhere.

Now restart your terminal or shell to make the installation effective.

??? question "Don't trust our link? Check the script!"
    You can check the installation `sh` script: [download](https://pixi.sh/install.sh) and the `ps1`: [download](https://pixi.sh/install.ps1).
    The scripts are open source and available on [GitHub](https://github.com/prefix-dev/pixi/tree/main/install).

!!! note "Don't forget to add autocompletion!"
    After installing Pixi, you can enable autocompletion for your shell.
    See the [Autocompletion](#autocompletion) section below for instructions.
## Update

Updating is as simple as installing, rerunning the installation script gets you the latest version.

```shell
pixi self-update
```
Or get a specific Pixi version using:
```shell
pixi self-update --version x.y.z
```

!!! note
    If you've used a package manager like `brew`, `mamba`, `conda`, `paru` etc. to install `pixi`
    you must use the built-in update mechanism. e.g. `brew upgrade pixi`.


## Alternative Installation Methods

Although we recommend installing Pixi through the above method we also provide additional installation methods.

### Homebrew

Pixi is available via homebrew. To install Pixi via homebrew simply run:

```shell
brew install pixi
```

### Windows Installer

We provide an `msi` installer on [our GitHub releases page](https://github.com/prefix-dev/pixi/releases/latest).
The installer will download Pixi and add it to the `PATH`.

### Winget

```
winget install prefix-dev.pixi
```

### Scoop

```
scoop install main/pixi
```

### Download From GitHub Releases

Pixi is a single executable and can be run without any external dependencies.
That means you can manually download the suitable archive for your architecture and operating system from our [GitHub releases](https://github.com/prefix-dev/pixi/releases), unpack it and then use it as is.
If you want `pixi` itself or the executables installed via `pixi global` to be available in your `PATH`, you have to add them manually.
The executables are located in [PIXI_HOME](reference/environment_variables.md)/bin.


### Install From Source

pixi is 100% written in Rust, and therefore it can be installed, built and tested with cargo.
To start using Pixi from a source build run:

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
its [compile steps](https://github.com/conda/rattler/tree/main#give-it-a-try).


## Installer Script Options

=== "Linux & macOS"

    The installation script has several options that can be manipulated through environment variables.

    | Variable             | Description                                                                        | Default Value         |
    |----------------------|------------------------------------------------------------------------------------|-----------------------|
    | `PIXI_VERSION`       | The version of Pixi getting installed, can be used to up- or down-grade.           | `latest`              |
    | `PIXI_HOME`          | The location of the binary folder.                                                 | `$HOME/.pixi`         |
    | `PIXI_ARCH`          | The architecture the Pixi version was built for.                                   | `uname -m`            |
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

    | Environment variable | Description                                                                       | Default Value               |
    |----------------------|-----------------------------------------------------------------------------------|-----------------------------|
    | `PIXI_VERSION`       | The version of Pixi getting installed, can be used to up- or down-grade.          | `latest`                    |
    | `PIXI_HOME`          | The location of the installation.                                                 | `$Env:USERPROFILE\.pixi`    |
    | `PIXI_NO_PATH_UPDATE`| If set, the `$PATH` will not be updated to add `pixi` to it.                      | `false`                     |

    For example, set the version:
    ```powershell
    $env:PIXI_VERSION='v0.18.0'; powershell -ExecutionPolicy Bypass -Command "iwr -useb https://pixi.sh/install.ps1 | iex"
    ```

## Autocompletion

To get autocompletion follow the instructions for your shell.
Afterwards, restart the shell or source the shell config file.

=== "Bash"
    Add the following to the end of `~/.bashrc`:
    ```bash title="~/.bashrc"
    eval "$(pixi completion --shell bash)"
    ```

=== "Zsh"
    Add the following to the end of `~/.zshrc`:

    ```zsh title="~/.zshrc"
    autoload -Uz compinit && compinit  # redundant with Oh My Zsh
    eval "$(pixi completion --shell zsh)"
    ```

=== "PowerShell"
    Add the following to the end of `Microsoft.PowerShell_profile.ps1`.
    You can check the location of this file by querying the `$PROFILE` variable in PowerShell.
    Typically the path is `~\Documents\PowerShell\Microsoft.PowerShell_profile.ps1` or
    `~/.config/powershell/Microsoft.PowerShell_profile.ps1` on -Nix.

    ```pwsh
    (& pixi completion --shell powershell) | Out-String | Invoke-Expression
    ```

=== "Fish"
    Add the following to the end of `~/.config/fish/config.fish`:

    ```fish title="~/.config/fish/config.fish"
    pixi completion --shell fish | source
    ```
=== "Nushell"
    Add the following to your Nushell config file (find it by running `$nu.config-path` in Nushell):

    ```nushell
    mkdir $"($nu.data-dir)/vendor/autoload"
    pixi completion --shell nushell | save --force $"($nu.data-dir)/vendor/autoload/pixi-completions.nu"
    ```

=== "Elvish"
    Add the following to the end of `~/.elvish/rc.elv`:

    ```elv title="~/.elvish/rc.elv"
    eval (pixi completion --shell elvish | slurp)
    ```

## Uninstall
Before un-installation you might want to delete any previous files pixi has installed.

1. Remove any cached data:
    ```shell
    pixi clean cache
    ```
2. Remove the environments from your pixi projects:
    ```shell
    cd path/to/project && pixi clean
    ```
3. Remove the `pixi` and it's global environments
    ```shell
    rm -r ~/.pixi
    ```
4. Remove the pixi binary from your `PATH`:
   - For Linux and macOS, remove `~/.pixi/bin` from your `PATH` in your shell configuration file (e.g., `~/.bashrc`, `~/.zshrc`).
   - For Windows, remove `%UserProfile%\.pixi\bin` from your `PATH` environment variable.
