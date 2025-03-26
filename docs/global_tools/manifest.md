# The Global Manifest

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

## Manifest locations

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


## Channels
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



## Dependencies

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


# Exposed executables

If tell `pixi global install`, under which name it will expose executables:

```shell
pixi global install --expose bird=bat bat
```

The manifest is modified like this:

```toml
[envs.bat]
channels = ["https://prefix.dev/conda-forge"]
dependencies = { bat = "*" }
exposed = { bird = "bat" }
```

This means that executable `bat` will be exposed under the name `bird`.
You can learn more about how executables are detected in the [concepts chapter](./concepts.md#automatically-exposed-executables).
