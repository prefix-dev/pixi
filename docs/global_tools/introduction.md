# Global Tools

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

## Install Dependencies From Source

Pixi global also allows you to install [Pixi packages](../build/getting_started.md).
Let's assume there's a C++ package we'd like to install globally from source.
First, it needs to have a package manifest:

```toml title="pixi.toml"
[package] 
name = "cpp_math"
version = "0.1.0"

[package.build]
backend = { name = "pixi-build-cmake", version = "*" }
```

If the source is on your machine, you can install it like this:

```shell
pixi global install --path /path/to/cpp_math
```

If the source resides in a git repository, you can access it like this:

```shell
pixi global install --git https://github.com/ORG_NAME/cpp_math.git
```

One has to take care if the source contains multiple outputs, see for example this recipe:

```yaml title="recipe.yaml"
recipe:
  name: multi-output
  version: "0.1.0"

outputs:
  - package:
      name: foobar
    build:
      script:
        - if: win
          then:
            - mkdir -p %PREFIX%\bin
            - echo @echo off > %PREFIX%\bin\foobar.bat
            - echo echo Hello from foobar >> %PREFIX%\bin\foobar.bat
          else:
            - mkdir -p $PREFIX/bin
            - echo "#!/usr/bin/env bash" > $PREFIX/bin/foobar
            - echo "echo Hello from foobar" >> $PREFIX/bin/foobar
            - chmod +x $PREFIX/bin/foobar

  - package:
      name: bizbar
    build:
      script:
        - if: win
          then:
            - mkdir -p %PREFIX%\bin
            - echo @echo off > %PREFIX%\bin\bizbar.bat
            - echo echo Hello from bizbar >> %PREFIX%\bin\bizbar.bat
          else:
            - mkdir -p $PREFIX/bin
            - echo "#!/usr/bin/env bash" > $PREFIX/bin/bizbar
            - echo "echo Hello from bizbar" >> $PREFIX/bin/bizbar
            - chmod +x $PREFIX/bin/bizbar
```

In this case, we have to specify which output we want to install:

```shell
pixi global install --path /path/to/package foobar
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
for file in ~/.pixi/completions/fish/*
    source $file
end
```

!!! note

    Completions of packages are installed as long as their binaries are exposed under the same name: e.g. `exposed = { git = "git" }`.

## Adding a Series of Tools at Once

Without specifying an environment, you can add multiple tools at once:
```shell
pixi global install pixi-pack rattler-build
```
This command generates the following entry in the manifest:
```toml
[envs.pixi-pack]
channels = ["conda-forge"]
dependencies= { pixi-pack = "*" }
exposed = { pixi-pack = "pixi-pack" }

[envs.rattler-build]
channels = ["conda-forge"]
dependencies = { rattler-build = "*" }
exposed = { rattler-build = "rattler-build" }
```
Creating two separate non-interfering environments, while exposing only the minimum required binaries.

## Creating a Data Science Sandbox Environment

You can create an environment with multiple tools using the following command:
```shell
pixi global install --environment data-science --expose jupyter --expose ipython jupyter numpy pandas matplotlib ipython
```
This command generates the following entry in the manifest:
```toml
[envs.data-science]
channels = ["conda-forge"]
dependencies = { jupyter = "*", ipython = "*" }
exposed = { jupyter = "jupyter", ipython = "ipython" }
```
In this setup, both `jupyter` and `ipython` are exposed from the `data-science` environment, allowing you to run:
```shell
> ipython
# Or
> jupyter lab
```
These commands will be available globally, making it easy to access your preferred tools without switching environments.

## Install Packages For a Different Platform

You can install packages for a different platform using the `--platform` flag.
This is useful when you want to install packages for a different platform, such as `osx-64` packages on `osx-arm64`.
For example, running this on `osx-arm64`:
```shell
pixi global install --platform osx-64 python
```
will create the following entry in the manifest:
```toml
[envs.python]
channels = ["conda-forge"]
platforms = ["osx-64"]
dependencies = { python = "*" }
# ...
```
