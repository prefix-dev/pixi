# Pixi Global

<div style="text-align:center">
 <video autoplay muted loop>
  <source src="https://github.com/user-attachments/assets/e94dc06f-75ae-4aa0-8830-7cb190d3f367" type="video/webm" />
  <p>Pixi global demo</p>
 </video>
</div>


With `pixi global`, users can manage globally installed tools in a way that makes them available from any directory.
This means that the Pixi environment will be placed in a global location, and the tools will be exposed to the system `PATH`, allowing you to run them from the command line.


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

## The Global Manifest

Since `v0.33.0` Pixi has a new manifest file that will be created in the global directory.
This file will contain the list of environments that are installed globally, their dependencies and exposed binaries.
The manifest can be edited, synced, checked in to a version control system, and shared with others.

Running the commands from the section before results in the following manifest:
```toml
version = 1

[envs.rattler-build]
channels = ["conda-forge"]
dependencies = { rattler-build = "*" }
exposed = { rattler-build = "rattler-build" }

[envs.ipython]
channels = ["conda-forge"]
dependencies = { ipython = "*", numpy = "*", matplotlib = "*" }
exposed = { ipython = "ipython", ipython3 = "ipython3" }

[envs.python]
channels = ["conda-forge"]
dependencies = { python = "3.12.*" } # (1)!
exposed = { py3 = "python" } # (2)!
```

1. Dependencies are the packages that will be installed in the environment. You can specify the version or use a wildcard.
2. The exposed binaries are the ones that will be available in the system path. In this case, `python` is exposed under the name `py3`.

### Manifest locations

The manifest can be found at the following locations depending on your operating system.
Run `pixi info`, to find the currently used manifest on your system.

=== "Linux"

    | **Priority** | **Location**                                             | **Comments**                                  |
    |--------------|----------------------------------------------------------|-----------------------------------------------|
    | 4            | `$PIXI_HOME/manifests/pixi-global.toml`                  | Global manifest in `PIXI_HOME`.               |
    | 3            | `$HOME/.pixi/manifests/pixi-global.toml`                 | Global manifest in user home directory.       |
    | 2            | `$XDG_CONFIG_HOME/pixi/manifests/pixi-global.toml`       | XDG compliant config directory.               |
    | 1            | `$HOME/.config/pixi/manifests/pixi-global.toml`          | Config directory.                             |

=== "macOS"

    | **Priority** | **Location**                                             | **Comments**                                  |
    |--------------|----------------------------------------------------------|-----------------------------------------------|
    | 3            | `$PIXI_HOME/manifests/pixi-global.toml`                  | Global manifest in `PIXI_HOME`.               |
    | 2            | `$HOME/.pixi/manifests/pixi-global.toml`                 | Global manifest in user home directory.       |
    | 1            | `$HOME/Library/Application Support/pixi/manifests/pixi-global.toml`| Config directory.                             |


=== "Windows"

    | **Priority** | **Location**                                             | **Comments**                                  |
    |--------------|----------------------------------------------------------|-----------------------------------------------|
    | 3            | `$PIXI_HOME\manifests/pixi-global.toml`                  | Global manifest in `PIXI_HOME`.               |
    | 2            | `%USERPROFILE%\.pixi\manifests\pixi-global.toml`         | Global manifest in user home directory.       |
    | 1            | `%APPDATA%\pixi\manifests\pixi-global.toml`                        | Config directory.                             |


!!! note
    If multiple locations exist, the manifest with the highest priority will be used.


### Channels
The channels are the conda channels that will be used to search for the packages.
There is a priority to these, so the first one will have the highest priority, if a package is not found in that channel the next one will be used.
For example, running:
```
pixi global install --channel conda-forge --channel bioconda snakemake
```
Results in the following entry in the manifest:
```toml
[envs.snakemake]
channels = ["conda-forge", "bioconda"]
dependencies = { snakemake = "*" }
exposed = { snakemake = "snakemake" }
```

More information on channels can be found [here](../advanced/channel_logic.md).

### Automatic Exposed

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

### Dependencies
Dependencies are the **Conda** packages that will be installed into your environment. For example, running:
```
pixi global install "python<3.12"
```
creates the following entry in the manifest:
```toml
[envs.vim]
channels = ["conda-forge"]
dependencies = { python = "<3.12" }
# ...
```
Typically, you'd specify just the tool you're installing, but you can add more packages if needed.
Defining the environment to install into will allow you to add multiple dependencies at once.
For example, running:
```shell
pixi global install --environment my-env git vim python
```
will create the following entry in the manifest:
```toml
[envs.my-env]
channels = ["conda-forge"]
dependencies = { git = "*", vim = "*", python = "*" }
# ...
```

You can `add` a dependency to an existing environment by running:
```shell
pixi global install --environment my-env package-a package-b
```
This will be added as dependencies to the `my-env` environment but won't auto expose the binaries from the new packages.

You can `remove` dependencies by running:
```shell
pixi global remove --environment my-env package-a package-b
```

### Trampolines

To increase efficiency, `pixi` uses *trampolines*—small, specialized binary files that manage configuration and environment setup before executing the main binary. The trampoline approach allows for skipping the execution of activation scripts that have a significant performance impact.

When you execute a global install binary, a trampoline performs the following sequence of steps:

* Each trampoline first reads a configuration file named after the binary being executed. This configuration file, in JSON format (e.g., `python.json`), contains key information about how the environment should be set up. The configuration file is stored in `.pixi/bin/trampoline_configuration`.
* Once the configuration is loaded and the environment is set, the trampoline executes the original binary with the correct environment settings.
* When installing a new binary, a new trampoline is placed in the `.pixi/bin` directory and is hard-linked to the `.pixi/bin/trampoline_configuration/trampoline_bin`. This optimizes storage space and avoids duplication of the same trampoline.

The trampoline will take care that the `PATH` contains the newest changes on your local `PATH` while avoiding to cache temporary `PATH` changes during installation.
If you want to control the base `PATH`, pixi considers you can set `export PIXI_BASE_PATH=$PATH` in your shell startup script.

### Shortcuts

Especially for graphical user interfaces it is useful to add shortcuts so that the operating system knows that about the application.
This way the application can show up in the start menu or be suggested when you want to open a file type the application supports.
If the package supports shortcuts, nothing has do be done from your side.
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



### Example: Adding a series of tools at once
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

### Example: Creating a Data Science Sandbox Environment
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

### Example: Install packages for a different platform
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


## Potential Future Features

### PyPI support

We could support packages from PyPI via a command like this:

```
pixi global install --pypi flask
```

### Lock file

A lock file is less important for global tools.
However, there is demand for it, and users that don't care about it should not be negatively impacted

### Multiple manifests

We could go for one default manifest, but also parse other manifests in the same directory.
The only requirement to be parsed as manifest is a `.toml` extension
In order to modify those with the `CLI` one would have to add an option `--manifest` to select the correct one.

- pixi-global.toml: Default
- pixi-global-company-tools.toml
- pixi-global-from-my-dotfiles.toml

It is unclear whether the first implementation already needs to support this.
At the very least we should put the manifest into its own folder like `~/.pixi/global/manifests/pixi-global.toml`

### No activation

The current `pixi global install` features `--no-activation`.
When this flag is set, `CONDA_PREFIX` and `PATH` will not be set when running the exposed executable.
This is useful when installing Python package managers or shells.

Assuming that this needs to be set per mapping, one way to expose this functionality would be to allow the following:

```toml
[envs.pip.exposed]
pip = { executable = "pip", activation = false }
```
