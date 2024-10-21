---
part: pixi
title: Environment
description: The resulting environment of a pixi installation.
---

# Environments

Pixi is a tool to manage virtual environments.
This document explains what an environment looks like and how to use it.

## Structure

A pixi environment is located in the `.pixi/envs` directory of the project.
This location is **not** configurable as it is a specific design decision to keep the environments in the project directory.
This keeps your machine and your project clean and isolated from each other, and makes it easy to clean up after a project is done.

If you look at the `.pixi/envs` directory, you will see a directory for each environment, the `default` being the one that is normally used, if you specify a custom environment the name you specified will be used.

```shell
.pixi
└── envs
    ├── cuda
    │   ├── bin
    │   ├── conda-meta
    │   ├── etc
    │   ├── include
    │   ├── lib
    │   ...
    └── default
        ├── bin
        ├── conda-meta
        ├── etc
        ├── include
        ├── lib
        ...
```

These directories are conda environments, and you can use them as such, but you cannot manually edit them, this should always go through the `pixi.toml`.
Pixi will always make sure the environment is in sync with the `pixi.lock` file.
If this is not the case then all the commands that use the environment will automatically update the environment, e.g. `pixi run`, `pixi shell`.

### Cleaning up

If you want to clean up the environments, you can simply delete the `.pixi/envs` directory, and pixi will recreate the environments when needed.

```shell
# either:
rm -rf .pixi/envs

# or per environment:
rm -rf .pixi/envs/default
rm -rf .pixi/envs/cuda
```

## Activation

An environment is nothing more than a set of files that are installed into a certain location, that somewhat mimics a global system install.
You need to activate the environment to use it.
In the most simple sense that mean adding the `bin` directory of the environment to the `PATH` variable.
But there is more to it in a conda environment, as it also sets some environment variables.

To do the activation we have multiple options:

- Use the `pixi shell` command to open a shell with the environment activated.
- Use the `pixi shell-hook` command to print the command to activate the environment in your current shell.
- Use the `pixi run` command to run a command in the environment.

Where the `run` command is special as it runs its own cross-platform shell and has the ability to run tasks.
More information about tasks can be found in the [tasks documentation](advanced_tasks.md).

Using the `pixi shell-hook` in pixi you would get the following output:

```shell
export PATH="/home/user/development/pixi/.pixi/envs/default/bin:/home/user/.local/bin:/home/user/bin:/usr/local/bin:/usr/local/sbin:/usr/bin:/home/user/.pixi/bin"
export CONDA_PREFIX="/home/user/development/pixi/.pixi/envs/default"
export PIXI_PROJECT_NAME="pixi"
export PIXI_PROJECT_ROOT="/home/user/development/pixi"
export PIXI_PROJECT_VERSION="0.12.0"
export PIXI_PROJECT_MANIFEST="/home/user/development/pixi/pixi.toml"
export CONDA_DEFAULT_ENV="pixi"
export PIXI_ENVIRONMENT_PLATFORMS="osx-64,linux-64,win-64,osx-arm64"
export PIXI_ENVIRONMENT_NAME="default"
export PIXI_PROMPT="(pixi) "
. "/home/user/development/pixi/.pixi/envs/default/etc/conda/activate.d/activate-binutils_linux-64.sh"
. "/home/user/development/pixi/.pixi/envs/default/etc/conda/activate.d/activate-gcc_linux-64.sh"
. "/home/user/development/pixi/.pixi/envs/default/etc/conda/activate.d/activate-gfortran_linux-64.sh"
. "/home/user/development/pixi/.pixi/envs/default/etc/conda/activate.d/activate-gxx_linux-64.sh"
. "/home/user/development/pixi/.pixi/envs/default/etc/conda/activate.d/libglib_activate.sh"
. "/home/user/development/pixi/.pixi/envs/default/etc/conda/activate.d/rust.sh"
```

It sets the `PATH` and some more environment variables. But more importantly it also runs activation scripts that are presented by the installed packages.
An example of this would be the [`libglib_activate.sh`](https://github.com/conda-forge/glib-feedstock/blob/52ba1944dffdb2d882d824d6548325155b58819b/recipe/scripts/activate.sh) script.
Thus, just adding the `bin` directory to the `PATH` is not enough.

## Traditional `conda activate`-like activation

If you prefer to use the traditional `conda activate`-like activation, you could use the `pixi shell-hook` command.

```shell
$ which python
python not found
$ eval "$(pixi shell-hook)"
$ (default) which python
/path/to/project/.pixi/envs/default/bin/python
```

!!! warning
    It is not encouraged to use the traditional `conda activate`-like activation, as deactivating the environment is not really possible. Use `pixi shell` instead.

### Using `pixi` with `direnv`

??? note "Installing direnv"

    Of course you can use `pixi` to install `direnv` globally. We recommend to run

    ```
    pixi global install direnv
    ```

    to install the latest version of `direnv` on your computer.

This allows you to use `pixi` in combination with `direnv`.
Enter the following into your `.envrc` file:

```shell title=".envrc"
watch_file pixi.lock # (1)!
eval "$(pixi shell-hook)" # (2)!
```

1. This ensures that every time your `pixi.lock` changes, `direnv` invokes the shell-hook again.
2. This installs if needed, and activates the environment. `direnv` ensures that the environment is deactivated when you leave the directory.

```shell
$ cd my-project
direnv: error /my-project/.envrc is blocked. Run `direnv allow` to approve its content
$ direnv allow
direnv: loading /my-project/.envrc
✔ Project in /my-project is ready to use!
direnv: export +CONDA_DEFAULT_ENV +CONDA_PREFIX +PIXI_ENVIRONMENT_NAME +PIXI_ENVIRONMENT_PLATFORMS +PIXI_PROJECT_MANIFEST +PIXI_PROJECT_NAME +PIXI_PROJECT_ROOT +PIXI_PROJECT_VERSION +PIXI_PROMPT ~PATH
$ which python
/my-project/.pixi/envs/default/bin/python
$ cd ..
direnv: unloading
$ which python
python not found
```

## Environment variables

The following environment variables are set by pixi, when using the `pixi run`, `pixi shell`, or `pixi shell-hook` command:

- `PIXI_PROJECT_ROOT`: The root directory of the project.
- `PIXI_PROJECT_NAME`: The name of the project.
- `PIXI_PROJECT_MANIFEST`: The path to the manifest file (`pixi.toml`).
- `PIXI_PROJECT_VERSION`: The version of the project.
- `PIXI_PROMPT`: The prompt to use in the shell, also used by `pixi shell` itself.
- `PIXI_ENVIRONMENT_NAME`: The name of the environment, defaults to `default`.
- `PIXI_ENVIRONMENT_PLATFORMS`: Comma separated list of platforms supported by the project.
- `CONDA_PREFIX`: The path to the environment. (Used by multiple tools that already understand conda environments)
- `CONDA_DEFAULT_ENV`: The name of the environment. (Used by multiple tools that already understand conda environments)
- `PATH`: We prepend the `bin` directory of the environment to the `PATH` variable, so you can use the tools installed in the environment directly.
- `INIT_CWD`: ONLY IN `pixi run`: The directory where the command was run from.

!!! note
    Even though the variables are environment variables these cannot be overridden. E.g. you can not change the root of the project by setting `PIXI_PROJECT_ROOT` in the environment.

## Solving environments

When you run a command that uses the environment, pixi will check if the environment is in sync with the `pixi.lock` file.
If it is not, pixi will solve the environment and update it.
This means that pixi will retrieve the best set of packages for the dependency requirements that you specified in the `pixi.toml` and will put the output of the solve step into the `pixi.lock` file.
Solving is a mathematical problem and can take some time, but we take pride in the way we solve environments, and we are confident that we can solve your environment in a reasonable time.
If you want to learn more about the solving process, you can read these:

- [Rattler(conda) resolver blog](https://prefix.dev/blog/the_new_rattler_resolver)
- [UV(PyPI) resolver blog](https://astral.sh/blog/uv-unified-python-packaging)

Pixi solves both the `conda` and `PyPI` dependencies, where the `PyPI` dependencies use the conda packages as a base, so you can be sure that the packages are compatible with each other.
These solvers are split between the [`rattler`](https://github.com/mamba-org/rattler) and [`uv`](https://github.com/astral-sh/uv) library, these control the heavy lifting of the solving process, which is executed by our custom SAT solver: [`resolvo`](https://github.com/mamba-org/resolvo).
`resolve` is able to solve multiple ecosystem like `conda` and `PyPI`. It implements the lazy solving process for `PyPI` packages, which means that it only downloads the metadata of the packages that are needed to solve the environment.
It also supports the `conda` way of solving, which means that it downloads the metadata of all the packages at once and then solves in one go.

For the `[pypi-dependencies]`, `uv` implements `sdist` building to retrieve the metadata of the packages, and `wheel` building to install the packages.
For this building step, `pixi` requires to first install `python` in the (conda)`[dependencies]` section of the `pixi.toml` file.
This will always be slower than the pure conda solves. So for the best pixi experience you should stay within the `[dependencies]` section of the `pixi.toml` file.

## Caching

Pixi caches all previously downloaded packages in a cache folder.
This cache folder is shared between all pixi projects and globally installed tools.

Normally the location would be the following
platform-specific default cache folder:

- Linux: `$XDG_CACHE_HOME/rattler` or `$HOME/.cache/rattler`
- macOS: `$HOME/Library/Caches/rattler`
- Windows: `%LOCALAPPDATA%\rattler`

This location is configurable by setting the `PIXI_CACHE_DIR` or `RATTLER_CACHE_DIR` environment variable.

When you want to clean the cache, you can simply delete the cache directory, and pixi will re-create the cache when needed.

The cache contains multiple folders concerning different caches from within pixi.

- `pkgs`: Contains the downloaded/unpacked `conda` packages.
- `repodata`: Contains the `conda` repodata cache.
- `uv-cache`: Contains the `uv` cache. This includes multiple caches, e.g. `built-wheels` `wheels` `archives`
- `http-cache`: Contains the `conda-pypi` mapping cache.
