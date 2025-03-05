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

This will create a pixi manifest which is a file called `pixi.toml`.
It describes the structure, dependencies and metadata of your workspace.

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/introduction/init/pixi.toml"
```

Let's add dependencies!

```bash
pixi add cowpy python
```

The dependencies are not only installed, but also tracked in the manifest.

```toml title="pixi.toml" hl_lines="6-8"
--8<-- "docs/source_files/pixi_workspaces/introduction/deps_add/pixi.toml"
```

We can now create a Python script which uses the `cowpy` library.

```py title="hello.py"
--8<-- "docs/source_files/pixi_workspaces/introduction/deps_add/hello.py"
```

The dependencies are installed in a pixi environment.
In order to run a command within an environment, we prefix it with `pixi run`.

```bash
pixi run python hello.py
```

```
 __________________
< Hello Pixi fans! >
 ------------------
     \   ^__^
      \  (oo)\_______
         (__)\       )\/\
           ||----w |
           ||     ||

```


You can also put this run command in a **task**.

```bash
$ pixi task add hello python hello.py
```

```toml title="pixi.toml" hl_lines="6-7"
--8<-- "docs/source_files/pixi_workspaces/introduction/task_add/pixi.toml"
```

After adding the task, you can run the task using its name.

```bash
$ pixi run start
```

```
 __________________
< Hello Pixi fans! >
 ------------------
     \   ^__^
      \  (oo)\_______
         (__)\       )\/\
           ||----w |
           ||     ||

```
