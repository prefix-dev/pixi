Welcome to the guide designed to ease your transition from `conda` or `mamba` to `pixi`. This document compares key commands and concepts between these tools, highlighting `pixi`'s unique approach to managing environments and packages. With `pixi`, you'll experience a workspace-based workflow, enhancing your development process, and allowing for easy sharing of your work.

## Why Pixi?

`Pixi` builds upon the foundation of the conda ecosystem, introducing a workspace-centric approach rather than focusing solely on environments. This shift towards workspaces offers a more organized and efficient way to manage dependencies and run code, tailored to modern development practices.

## Key Differences at a Glance

| Task                        | Conda/Mamba                                       | Pixi                                                                       |
| --------------------------- | ------------------------------------------------- | -------------------------------------------------------------------------- |
| Installation                | Requires an installer                             | Download and add to path (See [installation](../../))                      |
| Creating an Environment     | `conda create -n myenv -c conda-forge python=3.8` | `pixi init myenv` followed by `pixi add python=3.8`                        |
| Activating an Environment   | `conda activate myenv`                            | `pixi shell` within the workspace directory, or `pixi shell -w myenv`      |
| Deactivating an Environment | `conda deactivate`                                | `exit` from the `pixi shell`                                               |
| Running a Task              | `conda run -n myenv python my_program.py`         | `pixi run python my_program.py` (See [run](../../reference/cli/pixi/run/)) |
| Installing a Package        | `conda install numpy`                             | `pixi add numpy`                                                           |
| Uninstalling a Package      | `conda remove numpy`                              | `pixi remove numpy`                                                        |

No `base` environment

Conda has a base environment, which is the default environment when you start a new shell. **Pixi does not have a base environment**. And requires you to install the tools you need in the workspace or globally. Using `pixi global install bat` will install `bat` in a global environment, which is not the same as the `base` environment in conda.

Activating Pixi environment in the current shell

For some advanced use-cases, you can activate the environment in the current shell. This uses the `pixi shell-hook` which prints the activation script, which can be used without `pixi` itself.

```shell
~/myenv > eval "$(pixi shell-hook)"
```

## Environment vs Workspace

`Conda` and `mamba` focus on managing environments, while `pixi` emphasizes workspaces. In `pixi`, a workspace is a folder containing a [manifest](../../reference/pixi_manifest/)(`pixi.toml`/`pyproject.toml`) file that describes the workspace, a `pixi.lock` lock-file that describes the exact dependencies, and a `.pixi` folder that contains the environment.

This workspace-centric approach allows for easy sharing and collaboration, as the workspace folder contains all the necessary information to recreate the environment. It manages more than one environment for more than one platform in a single workspace, and allows for easy switching between them. (See [multiple environments](../../workspace/multi_environment/))

## Global environments

`conda` installs all environments in one global location. When this is important to you for filesystem reasons, you can use the [detached-environments](../../reference/pixi_configuration/#detached-environments) feature of pixi.

```shell
pixi config set detached-environments true
# or a specific location
pixi config set detached-environments /path/to/envs
```

This will make the installation of the environments go to the same folder.

`pixi` does have the `pixi global` command to install tools on your machine. (See [global](../../reference/cli/pixi/global/)) This is not a replacement for `conda` but works the same as [`pipx`](https://pipx.pypa.io/stable/) and [`condax`](https://mariusvniekerk.github.io/condax/). It creates a single isolated environment for the given requirement and installs the binaries into the global path.

```shell
pixi global install bat
bat pixi.toml
```

Never install `pip` with `pixi global`

Installations with `pixi global` get their own isolated environment. Installing `pip` with `pixi global` will create a new isolated environment with its own `pip` binary. Using that `pip` binary will install packages in the `pip` environment, making it unreachable from anywhere as you can't activate it.

## Named workspaces

`conda` provides the ability to perform actions on an environment by name or path to a prefix. For example,

```shell
# installs package into an environment named `myenv`
conda install --name myenv numpy
# installs a package into an environment that is located at `{_CONDA_ROOT}/envs/myenv`
conda install --prefix {_CONDA_ROOT}/envs/myenv numpy
```

`pixi` provides a similar functionality. You may register a named workspace using the `pixi workspace register` command.

```shell
pixi workspace register --name myproject --path /path/to/myproject
```

Creating a new named workspace

Registering a workspace in pixi is currently a two step process. First you must create a workspace, for example using the `pixi init` command. Then, you may add it to the workspace registry with the `pixi workspace register` command. This is in contrast to conda, which allows creating a new named environment, for example using the command `conda create --name myenv`.

Then, you may use the named workspace similar to how `conda` works

```shell
# adds a package into a workspace that has been registered with name `myproject`
pixi add numpy --workspace myproject
# adds a package into a workspace at the location `/path/to/myproject`
pixi add numpy --manifest-path /path/to/myproject
```

Use `pixi workspace register prune` to clean up disassociated workspaces

Your named workspace will be disassociated if you move the workspace path. To list all existing associations try running `pixi workspace register list`. To remove disassociated paths try running `pixi workspace register prune`.

## Automated switching

You can import `environment.yml` files into a Pixi workspace — see our [import tutorial](../../tutorials/import/).

Exporting your environment

If you are working with Conda users or systems, you can [export your environment to a `environment.yml`](../../reference/cli/pixi/workspace/export/) file to share them.

```shell
pixi workspace export conda-environment
```

Additionally you can export a [conda explicit specification](../../reference/cli/pixi/workspace/export/).

## Troubleshooting

Encountering issues? Here are solutions to some common problems when being used to the `conda` workflow:

- Dependency `is excluded due to strict channel priority not using this option from: 'https://conda.anaconda.org/conda-forge/'` This error occurs when the package is in multiple channels. `pixi` uses a strict channel priority. See [channel priority](../../advanced/channel_logic/) for more information.
- `pixi global install pip`, pip doesn't work. `pip` is installed in the global isolated environment. Use `pixi add pip` in a workspace to install `pip` in the workspace environment and use that workspace.
- `pixi global install <Any Library>` -> `import <Any Library>` -> `ModuleNotFoundError: No module named '<Any Library>'` The library is installed in the global isolated environment. Use `pixi add <Any Library>` in a workspace to install the library in the workspace environment and use that workspace.
