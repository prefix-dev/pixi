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

Check out our [installation docs](./advanced/installation.md) to learn about alternative installation methods, autocompletion and more.

## Getting Started


Initialize a new project and navigate to the project directory.

```bash
pixi init hello-world
cd hello-world
```

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/introduction/init/pixi.toml"
```

Add the dependencies you would like to use.

```bash
pixi add python
```

Create a file named `hello_world.py` in the directory and paste the following code into the file.

```py title="hello_world.py"
def hello():
    print("Hello World, from the revolution in package management.")

if __name__ == "__main__":
    hello()
```

Run the code inside the environment.

```bash
pixi run python hello_world.py
```

You can also put this run command in a **task**.

```bash
$ pixi task add hello python hello_world.py
```

After adding the task, you can run the task using its name.

```bash
$ pixi run hello
```
