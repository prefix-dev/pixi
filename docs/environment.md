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

If you look at the `.pixi/envs` directory, you will see a directory for each environment, the `default` being the one that is used if you only have one environment.

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
More information about tasks can be found in the [tasks documentation](advanced/advanced_tasks.md).

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
Thus, just adding the `bin` directory to the `PATH` is not enough.

## Environment variables
The following environment variables are set by pixi, when using the `pixi run`, `pixi shell`, or `pixi shell-hook` command:

- `PIXI_PROJECT_ROOT`: The root directory of the project.
- `PIXI_PROJECT_NAME`: The name of the project.
- `PIXI_PROJECT_MANIFEST`: The path to the manifest file (`pixi.toml`).
- `PIXI_PROJECT_VERSION`: The version of the project.
- `PIXI_PROMPT`: The prompt to use in the shell, also used by `pixi shell` itself.
- `PIXI_ENVIRONMENT_NAME`: The name of the environment, defaults to `default`.
- `PIXI_ENVIRONMENT_PLATFORMS`: The path to the environment.
- `CONDA_PREFIX`: The path to the environment. (Used by multiple tools that already understand conda environments)
- `CONDA_DEFAULT_ENV`: The name of the environment. (Used by multiple tools that already understand conda environments)
- `PATH`: We prepend the `bin` directory of the environment to the `PATH` variable, so you can use the tools installed in the environment directly.

!!! note
    Even though the variables are environment variables these cannot be overridden. E.g. you can not change the root of the project by setting `PIXI_PROJECT_ROOT` in the environment.

## Solving environments
When you run a command that uses the environment, pixi will check if the environment is in sync with the `pixi.lock` file.
If it is not, pixi will solve the environment and update it.
This means that pixi will retrieve the best set of packages for the dependency requirements that you specified in the `pixi.toml` and will put the output of the solve step into the `pixi.lock` file.
Solving is a mathematical problem and can take some time, but we take pride in the way we solve environments, and we are confident that we can solve your environment in a reasonable time.
If you want to learn more about the solving process, you can read these:

- [Rattler(conda) resolver blog](https://prefix.dev/blog/the_new_rattler_resolver)
- [Rip(PyPI) resolver blog](https://prefix.dev/blog/introducing_rip)

Pixi solves both the `conda` and `PyPI` dependencies, where the `PyPI` dependencies use the conda packages as a base, so you can be sure that the packages are compatible with each other.
These solvers are split between the `rattler` and `rip` library, these do the heavy lifting of the solving process.

## Caching
Pixi caches the packages used in the environment.
So if you have multiple projects that use the same packages, pixi will only download the packages once.

The cache is located in the `~/.cache/rattler/cache` directory by default.
This location is configurable by setting the `PIXI_CACHE_DIR` or `RATTLER_CACHE_DIR` environment variable.

When you want to clean the cache, you can simply delete the cache directory, and pixi will recreate the cache when needed.
