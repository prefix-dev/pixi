To install `pixi` you can run the following command in your terminal:

=== "Linux & macOS"
    ```bash
    curl -fsSL https://pixi.sh/install.sh | bash
    ```

    The above invocation will automatically download the latest version of `pixi`, extract it, and move the `pixi` binary to `~/.pixi/bin`.
    The script will also extend the `PATH` environment variable in the startup script of your shell to include `~/.pixi/bin`.
    This allows you to invoke `pixi` from anywhere.

=== "Windows"
    ```powershell
    powershell -ExecutionPolicy ByPass -c "irm -useb https://pixi.sh/install.ps1 | iex"
    ```

    The above invocation will automatically download the latest version of `pixi`, extract it, and move the `pixi` binary to `LocalAppData/pixi/bin`.
    The command will also add `LocalAppData/pixi/bin` to your `PATH` environment variable, allowing you to invoke `pixi` from anywhere.

!!! tip

    You might need to restart your terminal or source your shell for the changes to take effect.

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
The installer will download Pixi and add it to the path.

### Winget

```
winget install prefix-dev.pixi
```

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

    | Variable         | Environment variable | Description                                                                       | Default Value               |
    |------------------|----------------------|-----------------------------------------------------------------------------------|-----------------------------|
    | `PixiVersion`    | `PIXI_VERSION`       |The version of Pixi getting installed, can be used to up- or down-grade.           | `latest`                    |
    | `PixiHome`       | `PIXI_HOME`          | The location of the installation.                                                 | `$Env:USERPROFILE\.pixi`    |
    | `NoPathUpdate`   |                      | If set, the `$PATH` will not be updated to add `pixi` to it.                      |                             |

    For example, set the version using:

    ```powershell
    iwr -useb https://pixi.sh/install.ps1 | iex -Args "-PixiVersion v0.18.0"
    ```


## Autocompletion

To get autocompletion follow the instructions for your shell.
Afterwards, restart the shell or source the shell config file.


### Bash (default on most Linux systems)

Add the following to the end of `~/.bashrc`:

```bash title="~/.bashrc"

eval "$(pixi completion --shell bash)"
```
### Zsh (default on macOS)

Add the following to the end of `~/.zshrc`:


```zsh title="~/.zshrc"

autoload -Uz compinit && compinit  # redundant with Oh My Zsh
eval "$(pixi completion --shell zsh)"
```

### PowerShell (pre-installed on all Windows systems)

Add the following to the end of `Microsoft.PowerShell_profile.ps1`.
You can check the location of this file by querying the `$PROFILE` variable in PowerShell.
Typically the path is `~\Documents\PowerShell\Microsoft.PowerShell_profile.ps1` or
`~/.config/powershell/Microsoft.PowerShell_profile.ps1` on -Nix.

```pwsh
(& pixi completion --shell powershell) | Out-String | Invoke-Expression
```

### Fish

Add the following to the end of `~/.config/fish/config.fish`:

```fish title="~/.config/fish/config.fish"

pixi completion --shell fish | source
```

### Nushell

Add the following to your Nushell config file (find it by running `$nu.config-path` in Nushell):

```nushell
mkdir $"($nu.data-dir)/vendor/autoload"
pixi completion --shell nushell | save --force $"($nu.data-dir)/vendor/autoload/pixi-completions.nu"
```

### Elvish

Add the following to the end of `~/.elvish/rc.elv`:

```elv title="~/.elvish/rc.elv"

eval (pixi completion --shell elvish | slurp)
```
