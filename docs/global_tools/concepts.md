# Concepts

## Shortcuts

Especially for graphical user interfaces it is useful to add shortcuts so that the operating system knows that about the application.
This way the application can show up in the start menu or be suggested when you want to open a file type the application supports.
If the package supports shortcuts, nothing has to be done from your side.
Simply executing `pixi global install` will do the trick.
For example, `pixi global install mss` will lead to the following manifest:

```toml
[envs.mss]
channels = ["https://prefix.dev/conda-forge"]
dependencies = { mss = "*" }
exposed = { ... }
shortcuts = ["mss"]
```

Note the `shortcuts` entry.
If it's present, `pixi` will install the shortcut for the `mss` package.
If you want to package an application yourself that would benefit from this, you can check out the corresponding [documentation](https://conda.github.io/menuinst/).


## Completions

When you work in a terminal, you are using a shell and shells can process completions of command line tools.
That means if you have the tool `rg` and its completions installed on your system, type "rg -" and press `<TAB>`,
your shell will present you all the flags `rg` offers.
If the tool you installed via `pixi global` contains completions they will be automatically installed, as long as their binaries are exposed under the same name: e.g. `exposed = { rg = "rg" }`.
At the moment, only Linux and macOS are supported.

You can then find the completions under `~/.pixi/completions` or `$PIXI_HOME/completions` if `$PIXI_HOME` is set.

Depending on your shell, you can load the completions in your startup script:

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


## Automatically Exposed Executables

There is some added automatic behavior, if you install a package with the same name as the environment, it will be exposed with the same name.
Even if the binary name is only exposed through dependencies of the package
For example, running:
```
pixi global install ansible
```
will create the following entry in the manifest:
```toml
[envs.ansible]
channels = ["conda-forge"]
dependencies = { ansible = "*" }
exposed = { ansible = "ansible" } # (1)!
```

1. The `ansible` binary is exposed even though it is installed by a dependency of `ansible`, the `ansible-core` package.

It's also possible to expose an executable which is located in a nested directory.
For example dotnet.exe executable is located in a dotnet folder,
to expose `dotnet` you must specify its relative path :

```
pixi global install dotnet --expose dotnet=dotnet\dotnet
```

Which will create the following entry in the manifest:
```toml
[envs.dotnet]
channels = ["conda-forge"]
dependencies = { dotnet = "*" }
exposed = { dotnet = 'dotnet\dotnet' }
```

## Trampolines

To increase efficiency, `pixi` uses *trampolines*—small, specialized binary files that manage configuration and environment setup before executing the main binary. The trampoline approach allows for skipping the execution of activation scripts that have a significant performance impact.

When you execute a global install binary, a trampoline performs the following sequence of steps:

* Each trampoline first reads a configuration file named after the binary being executed. This configuration file, in JSON format (e.g., `python.json`), contains key information about how the environment should be set up. The configuration file is stored in `.pixi/bin/trampoline_configuration`.
* Once the configuration is loaded and the environment is set, the trampoline executes the original binary with the correct environment settings.
* When installing a new binary, a new trampoline is placed in the `.pixi/bin` directory and is hard-linked to the `.pixi/bin/trampoline_configuration/trampoline_bin`. This optimizes storage space and avoids duplication of the same trampoline.

The trampoline will take care that the `PATH` contains the newest changes on your local `PATH` while avoiding to cache temporary `PATH` changes during installation.
If you want to control the base `PATH`, pixi considers you can set `export PIXI_BASE_PATH=$PATH` in your shell startup script.
