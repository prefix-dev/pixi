# Introduction

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

Check out our [installation docs](./advanced/installation.md) to learn about alternative installation methods, autocompletion and more.

## Getting Started



Initialize a new project and navigate to the project directory.

```console
$ pixi init pixi-hello-world
✔ Created /path/to/pixi-hello-world/pixi.toml

$ cd pixi-hello-world
```

Add the dependencies you would like to use.

```console
$ pixi add python
✔ Added python >=3.13.2,<3.14
```

Create a file named `hello_world.py` in the directory and paste the following code into the file.

```py title="hello_world.py"
def hello():
    print("Hello World, from the revolution in package management.")

if __name__ == "__main__":
    hello()
```

Run the code inside the environment.

```console
$ pixi run python hello_world.py
Hello World, from the revolution in package management.
```

You can also put this run command in a **task**.

```console
$ pixi task add hello python hello_world.py
```

After adding the task, you can run the task using its name.

```console
$ pixi run hello
Hello World, from the revolution in package management.
```
