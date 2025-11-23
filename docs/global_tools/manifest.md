# The Global Manifest

This global manifest contains the list of environments that are installed globally, their dependencies and exposed binaries.
It can be edited, synced, checked in to a version control system, and shared with others.

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

## Lock Files

Pixi global creates and maintains a `pixi-global.lock` file alongside your manifest for reproducible installations.

When you install or update global tools, pixi automatically creates a `pixi-global.lock` file in the same directory as your manifest. This lock file contains the exact resolved package versions for all your global environments, ensuring reproducible installations across different machines.

### Benefits

- **Reproducibility**: Share your `pixi-global.toml` and `pixi-global.lock` files with your team to ensure everyone has identical tool versions
- **Version Control**: Commit both files to version control for consistent team environments
- **Automatic Updates**: The lock file is automatically updated when you add, remove, or update packages
- **Cross-Platform**: Lock files work across different operating systems, maintaining separate resolutions per platform where needed

### Example Workflow

```shell
# Install a tool - creates/updates the lock file
pixi global install bat

# Share your manifest and lock file with others
git add ~/.pixi/manifests/pixi-global.{toml,lock}
git commit -m "Add bat tool"

# On another machine, sync to get identical versions
pixi global sync
```

### Lock File Location

The lock file is stored in the same directory as your manifest:

=== "Linux"
    - `$PIXI_HOME/manifests/pixi-global.lock`
    - `$HOME/.pixi/manifests/pixi-global.lock`
    - `$XDG_CONFIG_HOME/pixi/manifests/pixi-global.lock`
    - `$HOME/.config/pixi/manifests/pixi-global.lock`

=== "macOS"
    - `$PIXI_HOME/manifests/pixi-global.lock`
    - `$HOME/.pixi/manifests/pixi-global.lock`
    - `$HOME/Library/Application Support/pixi/manifests/pixi-global.lock`

=== "Windows"
    - `$PIXI_HOME\manifests\pixi-global.lock`
    - `%USERPROFILE%\.pixi\manifests\pixi-global.lock`
    - `%APPDATA%\pixi\manifests\pixi-global.lock`

!!! tip
    You typically don't need to edit the lock file manually. Just use `pixi global install`, `add`, `remove`, or `sync` commands, and pixi will update it automatically.

## Manifest locations

The manifest can be found at the following locations depending on your operating system.
Run [`pixi info`](../reference/cli/pixi/info.md), to find the currently used manifest on your system.

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

The `channels` key describes the Conda channels that will be used to download the packages.
There is a priority to these, so the first one will have the highest priority.
If a package is not found in that channel the next one will be used.
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

Dependencies are the Conda packages that will be installed into your environment. For example, running:
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

You can [`add`](../reference/cli/pixi/global/add.md) dependencies to an existing environment by running:
```shell
pixi global add --environment my-env package-a package-b
```

They will be added as dependencies to the `my-env` environment but won't auto expose the binaries from the new packages.

You can [`remove`](../reference/cli/pixi/global/remove.md) dependencies by running:

```shell
pixi global remove --environment my-env package-a package-b
```


## Exposed executables

One can instruct `pixi global install`, under which name it will expose executables:

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

### Automatically Exposed Executables

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

## Shortcuts

Especially for graphical user interfaces it is useful to add shortcuts.
This way the application shows up in the start menu or is suggested when you want to open a file type the application supports.
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
This means, the application will show up in the start menu.
If you want to package an application yourself that would benefit from this, you can check out the corresponding [documentation](https://conda.github.io/menuinst/).
