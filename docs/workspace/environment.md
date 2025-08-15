Pixi is a tool to manage virtual environments.
This document explains what an environment looks like and how to use it.

## Activation

An environment is nothing more than a set of files that are installed into a certain location, that somewhat mimics a global system install.
You need to activate the environment to use it.
In the most simple sense that mean adding the `bin` directory of the environment to the `PATH` variable.
But there is more to it in a conda environment, as it also sets some environment variables.

To do the activation we have multiple options:

- `pixi shell`: start a shell with the environment activated.
- `pixi shell-hook`: print the command to activate the environment in your current shell.
- `pixi run` run a command or [task](./advanced_tasks.md) in the environment.

Where the `run` command is special as it runs its own cross-platform shell and has the ability to run tasks.
More information about tasks can be found in the [tasks documentation](./advanced_tasks.md).

Using the `pixi shell-hook` in Pixi you would get the following output:

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

You can modify the activation with the `activation` table in the manifest, you can add more activation scripts or inject environment variables into the activation scripts.
```toml
--8<-- "docs/source_files/pixi_tomls/activation.toml:activation"
```
Find the reference for the `activation` table [here](../reference/pixi_manifest.md#the-activation-table).

--8<-- "docs/partials/conda-style-activation.md"


## Structure

A Pixi environment is located in the `.pixi/envs` directory of the workspace by default.
This keeps your machine and your workspace clean and isolated from each other, and makes it easy to clean up after a workspace is done.
While this structure is generally recommended, environments can also be stored outside of workspace directories by enabling [detached environments](../reference/pixi_configuration.md#detached-environments).

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

### Environment Installation Metadata
On environment installation, Pixi will write a small file to the environment that contains some metadata about installation.
This file is called `pixi` and is located in the `conda-meta` folder of the environment.
This file contains the following information:

- `manifest_path`: The path to the manifest file that describes the workspace used to create this environment
- `environment_name`: The name of the environment
- `pixi_version`: The version of Pixi that was used to create this environment
- `environment_lock_file_hash`: The hash of the `pixi.lock` file that was used to create this environment

```json
{
  "manifest_path": "/home/user/dev/pixi/pixi.toml",
  "environment_name": "default",
  "pixi_version": "0.34.0",
  "environment_lock_file_hash": "4f36ee620f10329d"
}
```

The `environment_lock_file_hash` is used to check if the environment is in sync with the `pixi.lock` file.
If the hash of the `pixi.lock` file is different from the hash in the `pixi` file, Pixi will update the environment.

This is used to speedup activation, in order to trigger a full revalidation and installation use `pixi install` or `pixi reinstall`.
A broken environment would typically not be found with a hash comparison, but a revalidation would reinstall the environment.
By default, all lock file modifying commands will always use the revalidation and on `pixi install` it always revalidates.

### Cleaning up

If you want to clean up the environments, you can simply delete the `.pixi/envs` directory, and Pixi will recreate the environments when needed.

```shell
pixi clean
# or manually:
rm -rf .pixi/envs

# or per environment:
pixi clean --environment cuda
# or manually:
rm -rf .pixi/envs/default
rm -rf .pixi/envs/cuda
```

## Solving environments

When you run a command that uses the environment, Pixi will check if the environment is in sync with the `pixi.lock` file.
If it is not, Pixi will solve the environment and update it.
This means that Pixi will retrieve the best set of packages for the dependency requirements that you specified in the `pixi.toml` and will put the output of the solve step into the `pixi.lock` file.
Solving is a mathematical problem and can take some time, but we take pride in the way we solve environments, and we are confident that we can solve your environment in a reasonable time.
If you want to learn more about the solving process, you can read these:

- [Rattler(conda) resolver blog](https://prefix.dev/blog/the_new_rattler_resolver)
- [UV(PyPI) resolver blog](https://astral.sh/blog/uv-unified-python-packaging)

Pixi solves both the `conda` and `PyPI` dependencies, where the `PyPI` dependencies use the conda packages as a base, so you can be sure that the packages are compatible with each other.
These solvers are split between the [`rattler`](https://github.com/conda/rattler) and [`uv`](https://github.com/astral-sh/uv) library, these control the heavy lifting of the solving process, which is executed by our custom SAT solver: [`resolvo`](https://github.com/mamba-org/resolvo).
`resolvo` is able to solve multiple ecosystem like `conda` and `PyPI`. It implements the lazy solving process for `PyPI` packages, which means that it only downloads the metadata of the packages that are needed to solve the environment.
It also supports the `conda` way of solving, which means that it downloads the metadata of all the packages at once and then solves in one go.

For the `[pypi-dependencies]`, `uv` implements `sdist` building to retrieve the metadata of the packages, and `wheel` building to install the packages.
For this building step, `pixi` requires to first install `python` in the (conda)`[dependencies]` section of the `pixi.toml` file.
This will always be slower than the pure conda solves. So for the best Pixi experience you should stay within the `[dependencies]` section of the `pixi.toml` file.

## Caching packages

Pixi caches all previously downloaded packages in a cache folder.
This cache folder is shared between all Pixi projects and globally installed tools.

Normally the location would be the following
platform-specific default cache folder:

- Linux: `$XDG_CACHE_HOME/rattler` or `$HOME/.cache/rattler`
- macOS: `$HOME/Library/Caches/rattler`
- Windows: `%LOCALAPPDATA%\rattler`

This location is configurable by setting the `PIXI_CACHE_DIR` or `RATTLER_CACHE_DIR` environment variable.

When you want to clean the cache, you can simply delete the cache directory, and Pixi will re-create the cache when needed.

The cache contains multiple folders concerning different caches from within pixi.

- `pkgs`: Contains the downloaded/unpacked `conda` packages.
- `repodata`: Contains the `conda` repodata cache.
- `uv-cache`: Contains the `uv` cache. This includes multiple caches, e.g. `built-wheels` `wheels` `archives`
- `http-cache`: Contains the `conda-pypi` mapping cache.
