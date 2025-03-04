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

    The script will also update your `~/.bashrc` or `~/.zshrc` to include `~/.pixi/bin` in your PATH, allowing you to invoke the `pixi` command from anywhere.

=== "Windows"
    `PowerShell`:
    ```powershell
    powershell -ExecutionPolicy ByPass -c "irm -useb https://pixi.sh/install.ps1 | iex"
    ```
    Changing the [execution policy](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.core/about/about_execution_policies?view=powershell-7.4#powershell-execution-policies) allows running a script from the internet.
    Check the script you would be running with:
    ```powershell
    powershell -c "irm -useb https://pixi.sh/install.ps1 | more"
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

Check out our [advanced installation docs](./advanced/installation.md) to learn about how to install autocompletion, alternative installation methods and installer script options.


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
