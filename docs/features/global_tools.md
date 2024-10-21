# Pixi Global Tool Environment Installation

With `pixi global`, users can manage globally installed tools in a way that makes them available from any directory.
This means that the pixi environment will be placed in a global location, and the tools will be exposed to the system `PATH`, allowing you to run them from the command line.

## The Global Manifest
Since `v0.33.0` pixi has a new manifest file that will be created in the global directory.
This file will contain the list of environments that are installed globally, their dependencies and exposed binaries.
The manifest can be edited, synced, checked in to a version control system, and shared with others.


A simple version looks like this:
```toml
[envs.vim]
channels = ["conda-forge"]
dependencies = { vim = "*" } # (1)!
exposed = { vimdiff = "vimdiff", vim = "vim" } # (2)!

[envs.gh]
channels = ["conda-forge"]
dependencies = { gh = "*" }
exposed = { gh = "gh" }

[envs.python]
channels = ["conda-forge"]
dependencies = { python = ">=3.10,<3.11" }
exposed = { python310 = "python" } # (3)!
```

1. Dependencies are the packages that will be installed in the environment. You can specify the version or use a wildcard.
2. The exposed binaries are the ones that will be available in the system path. `vim` has multiple and all of them are exposed.
3. Here python is exposed as `python310` to avoid conflicts with other python installations. You can give it any name you want.

### Manifest locations

The manifest can be found at the following locations depending on your operation system.

=== "Linux"

    | **Priority** | **Location**                                                           | **Comments**                                                                       |
    |--------------|------------------------------------------------------------------------|------------------------------------------------------------------------------------|
    | 1            | `$HOME/.pixi/manifests/pixi-global.toml`                               | User-specific manifest                                                             |
    | 2            | `$PIXI_HOME/manifests/pixi-global.toml`                                | Global manifest in the user home directory. `PIXI_HOME` defaults to `~/.pixi`      |

=== "macOS"

    | **Priority** | **Location**                                                           | **Comments**                                                                       |
    |--------------|------------------------------------------------------------------------|------------------------------------------------------------------------------------|
    | 1            | `$HOME/.pixi/manifests/pixi-global.toml`                               | User-specific manifest                                                             |
    | 2            | `$PIXI_HOME/manifests/pixi-global.toml`                                | Global manifest in the user home directory. `PIXI_HOME` defaults to `~/.pixi`      |

=== "Windows"

    | **Priority** | **Location**                                                           | **Comments**                                                                                   |
    |--------------|------------------------------------------------------------------------|------------------------------------------------------------------------------------------------|
    | 1            | `%USERPROFILE%\.pixi\manifests\pixi-global.toml`                       | User-specific manifest                                                                         |
    | 2            | `$PIXI_HOME\manifests/pixi-global.toml`                                | Global manifest in the user home directory. `PIXI_HOME` defaults to `%USERPROFILE%/.pixi`      |

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

More information on channels can be found [here](../advanced/channel_priority.md).

### Exposed
The exposed binaries are the ones that will be available in the system `PATH`.
This is useful when the package has multiple binaries, but you want to get a select few, or you want to expose it with a different name.
For example, the `python` package has multiple binaries, but you only want to expose the interpreter as `py3`.
Running:
```
pixi global expose add --environment python py3=python3
```
will create the following entry in the manifest:
```toml
[envs.python]
channels = ["conda-forge"]
dependencies = { python = ">=3.10,<3.11" }
exposed = { py3 = "python3" }
```
Now you can run `py3` to start the python interpreter.
```shell
py3 -c "print('Hello World')"
```

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
