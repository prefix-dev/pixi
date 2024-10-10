# Transitioning from the `conda` or `mamba` to `pixi`
Welcome to the guide designed to ease your transition from `conda` or `mamba` to `pixi`.
This document compares key commands and concepts between these tools, highlighting `pixi`'s unique approach to managing environments and packages.
With `pixi`, you'll experience a project-based workflow, enhancing your development process, and allowing for easy sharing of your work.

## Why Pixi?

`Pixi` builds upon the foundation of the conda ecosystem, introducing a project-centric approach rather than focusing solely on environments.
This shift towards projects offers a more organized and efficient way to manage dependencies and run code, tailored to modern development practices.

## Key Differences at a Glance

| Task                        | Conda/Mamba                                       | Pixi                                                                 |
|-----------------------------|---------------------------------------------------|----------------------------------------------------------------------|
| Installation                | Requires an installer                             | Download and add to path (See [installation](../index.md))           |
| Creating an Environment     | `conda create -n myenv -c conda-forge python=3.8` | `pixi init myenv` followed by `pixi add python=3.8`                  |
| Activating an Environment   | `conda activate myenv`                            | `pixi shell` within the project directory                            |
| Deactivating an Environment | `conda deactivate`                                | `exit` from the `pixi shell`                                         |
| Running a Task              | `conda run -n myenv python my_program.py`         | `pixi run python my_program.py` (See [run](../reference/cli.md#run)) |
| Installing a Package        | `conda install numpy`                             | `pixi add numpy`                                                     |
| Uninstalling a Package      | `conda remove numpy`                              | `pixi remove numpy`                                                  |

!!! warn "No `base` environment"
    Conda has a base environment, which is the default environment when you start a new shell.
    **Pixi does not have a base environment**. And requires you to install the tools you need in the project or globally.
    Using `pixi global install bat` will install `bat` in a global environment, which is not the same as the `base` environment in conda.

??? tip "Activating pixi environment in the current shell"
    For some advanced use-cases, you can activate the environment in the current shell.
    This uses the `pixi shell-hook` which prints the activation script, which can be used to activate the environment in the current shell without `pixi` itself.
    ```shell
    ~/myenv > eval "$(pixi shell-hook)"
    ```

## Environment vs Project
`Conda` and `mamba` focus on managing environments, while `pixi` emphasizes projects.
In `pixi`, a project is a folder containing a [manifest](../reference/project_configuration.md)(`pixi.toml`/`pyproject.toml`) file that describes the project, a `pixi.lock` lock-file that describes the exact dependencies, and a `.pixi` folder that contains the environment.

This project-centric approach allows for easy sharing and collaboration, as the project folder contains all the necessary information to recreate the environment.
It manages more than one environment for more than one platform in a single project, and allows for easy switching between them. (See [multiple environments](../features/multi_environment.md))

## Global environments
`conda` installs all environments in one global location.
When this is important to you for filesystem reasons, you can use the [detached-environments](../reference/pixi_configuration.md#detached-environments) feature of pixi.
```shell
pixi config set detached-environments true
# or a specific location
pixi config set detached-environments /path/to/envs
```
This doesn't allow you to activate the environments using `pixi shell -n` but it will make the installation of the environments go to the same folder.

`pixi` does have the `pixi global` command to install tools on your machine. (See [global](../reference/cli.md#global))
This is not a replacement for `conda` but works the same as [`pipx`](https://pipx.pypa.io/stable/) and [`condax`](https://mariusvniekerk.github.io/condax/).
It creates a single isolated environment for the given requirement and installs the binaries into the global path.
```shell
pixi global install bat
bat pixi.toml
```

!!! warn "Never install `pip` with `pixi global`"
Installations with `pixi global` get their own isolated environment.
Installing `pip` with `pixi global` will create a new isolated environment with its own `pip` binary.
Using that `pip` binary will install packages in the `pip` environment, making it unreachable form anywhere as you can't activate it.


## Automated switching
With `pixi` you can import `environment.yml` files into a pixi project. (See [import](../reference/cli.md#init))
```shell
pixi init --import environment.yml
```
This will create a new project with the dependencies from the `environment.yml` file.

??? tip "Exporting your environment"
    If you are working with Conda users or systems, you can [export your environment to a `environment.yml`](../reference/cli.md#project-export-conda_environment) file to share them.
    ```shell
    pixi project export conda
    ```
    Additionally you can export a [conda explicit specification](../reference/cli.md#project-export-conda_explicit_spec).

## Troubleshooting
Encountering issues? Here are solutions to some common problems when being used to the `conda` workflow:

- Dependency `is excluded because due to strict channel priority not using this option from: 'https://conda.anaconda.org/conda-forge/'`
  This error occurs when the package is in multiple channels. `pixi` uses a strict channel priority. See [channel priority](../advanced/channel_priority.md) for more information.
- `pixi global install pip`, pip doesn't work.
  `pip` is installed in the global isolated environment. Use `pixi add pip` in a project to install `pip` in the project environment and use that project.
- `pixi global install <Any Library>` -> `import <Any Library>` -> `ModuleNotFoundError: No module named '<Any Library>'`
   The library is installed in the global isolated environment. Use `pixi add <Any Library>` in a project to install the library in the project environment and use that project.
