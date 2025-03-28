# Pixi Global

<div style="text-align:center">
 <video autoplay muted loop>
  <source src="https://github.com/user-attachments/assets/e94dc06f-75ae-4aa0-8830-7cb190d3f367" type="video/webm" />
  <p>Pixi global demo</p>
 </video>
</div>


With `pixi global`, users can manage globally installed tools in a way that makes them available from any directory.
This means that the Pixi environment will be placed in a global location, and the tools will be exposed to the system `PATH`, allowing you to run them from the command line.
Some packages, especially those with graphical user interfaces, will also add start menu entries.


## Basic Usage

Running the following command installs [`rattler-build`](https://prefix-dev.github.io/rattler-build/latest/) on your system.

```bash
pixi global install rattler-build
```

What's great about `pixi global` is that, by default, it isolates each package in its own environment, exposing only the necessary entry points.
This means you don't have to worry about removing a package and accidentally breaking seemingly unrelated packages.
This behavior is quite similar to that of [`pipx`](https://pipx.pypa.io/latest/installation/).

However, there are times when you may want multiple dependencies in the same environment.
For instance, while `ipython` is really useful on its own, it becomes much more useful when `numpy` and `matplotlib` are available when using it.

Let's execute the following command:

```bash
pixi global install ipython --with numpy --with matplotlib
```

`numpy` exposes executables, but since it's added via `--with` it's executables are not being exposed.

Importing `numpy` and `matplotlib` now works as expected.
```bash
ipython -c 'import numpy; import matplotlib'
```

At some point, you might want to install multiple versions of the same package on your system.
Since they will be all available on the system `PATH`, they need to be exposed under different names.

Let's check out the following command:
```bash
pixi global install --expose py3=python "python=3.12"
```

By specifying `--expose` we specified that we want to expose the executable `python` under the name `py3`.
The package `python` has more executables, but since we specified `--exposed` they are not auto-exposed.

You can run `py3` to start the python interpreter.
```shell
py3 -c "print('Hello World')"
```

## Shell Completions

When you work in a terminal, you are using a shell and shells can process completions of command line tools.
The process works like this: you type "git -" in your terminal and press `<TAB>`.
Then, your shell will present you all the flags `git` offers.
However, that only works if you have the completions installed for the tool in question.
If the tool you installed via `pixi global` contains completions they will be automatically installed.
At the moment, only Linux and macOS are supported.


First install a tool with `pixi global`:

```shell
pixi global install git
```

The completions can be found under [`$PIXI_HOME`](../reference/environment_variables.md)`/completions`.

You can then load the completions in the startup script of your shell:

```bash title="~/.bashrc"
# bash, default on most Linux distributions
source ~/.pixi/completions/bash/*
```

```zsh title="~/.zshrc"
# zsh, default on macOS
fpath+=(~/.pixi/completions/zsh)
autoload -Uz compinit
compinit
```

```fish title="~/.config/fish/config.fish"
# fish
for file in ~/.pixi/completions/fish
    source $file
end
```

!!! note

    Completions of packages are installed as long as their binaries are exposed under the same name: e.g. `exposed = { git = "git" }`.
