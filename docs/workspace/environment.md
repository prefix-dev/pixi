Pixi is a tool to manage environments.
This document explains what an environment looks like and how to use it.

## Activation

An environment is nothing more than a set of files that are installed into a certain location, that somewhat mimics a global system install.
You need to activate the environment to use it.
In the most simple sense that mean adding the `bin` directory of the environment to the `PATH` variable.
But there is more to it in a conda environment, as it also sets some environment variables.

To do the activation we have multiple options:

- `pixi shell`: start a shell with the environment activated.
- `pixi shell-hook`: print the command to activate the environment in your current shell.
- `pixi run` run a command or [task](./advanced_tasks.md) in the default environment.

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

Shell used for activation:
- On Windows, Pixi executes activation under `cmd.exe`.
- On Linux and macOS, Pixi executes activation under `bash`.

This affects both `[activation.env]` and `activation.scripts`: they are applied by the platform's shell during activation, before any task runs.

You can modify the activation with the `activation` table in the manifest, you can add more activation scripts or inject environment variables into the activation scripts.
```toml
--8<-- "docs/source_files/pixi_tomls/activation.toml:activation"
```
Find the reference for the `activation` table [here](../reference/pixi_manifest.md#the-activation-table).

### Default Activation Behavior

Besides `[activation.env]` and `activation.scripts`, pixi also prepends some values to `PATH` implicitly.

For Windows:

- `$PREFIX/Library/mingw-w64/bin`
- `$PREFIX/Library/usr/bin`
- `$PREFIX/Library/bin`
- `$PREFIX/Scripts`
- `$PREFIX/bin`

For other systems:

- `$PREFIX/bin`

Under some very edge cases, this will cause unexpected result. Here is a example `pixi-global.toml` on Windows and we assume `$PIXI_HOME/bin`
already exists in `PATH`.

```toml
version = 1
[envs.tools]
channels = ["conda-forge"]
platform = "win-64"
[envs.tools.dependencies]
powershell = "*"
git = "*"
ripgrep = "*"
[envs.tools.exposed]
pwsh = "pwsh"
git = "git"
grep = "rg" # (1)!
```

1. I prefer `ripgrep` than the GNU `grep`


After install the global tools with `pixi global update`, there comes two problems if you use the `pwsh` as your shell, though its valid to pixi.

1. Pixi silently **"exposes"** all GNU coreutils by prepending `$PIXI_HOME/envs/tool/Library/mingw-w64/bin` to `PATH`, and `pwsh` inherits `PATH`.
2. The `grep` in `pwsh` is not the `ripgrep` one, but the GNU `grep` installed with `git`, because `$PIXI_HOME/envs/tool/Library/mingw-w64/bin`
   (where the GNU `grep` locates) in `PATH` has the higher priority than `$PIXI_HOME/bin`.

The solution is to install `powershell` in a isolated environment. Though pixi prepend the `$PIXI_HOME/envs/shell/Library/mingw-w64/bin` to `PATH`, but there is nothing under it.

```toml
version = 1
[envs.tools]
channels = ["conda-forge"]
platform = "win-64"
[envs.tools.dependencies]
git = "*"
ripgrep = "*"
[envs.tools.exposed]
git = "git"
grep = "rg"

[envs.shell]
channels = ["conda-forge"]
platform = "win-64"
[envs.shell.dependencies]
powershell = "*"
[envs.shell.exposed]
pwsh = "pwsh"
```

!!! tip "danger"
    - **Use Windows** More paths prepends to `PATH` than other system and there might be some executables with the same name in these
      paths.
    - **Use Global Tools** Global tools allow you to modify the executable name, which will be omitted if there is a executable or shell
      alias with the same name and has higher priority than the one in `$PIXI_HOME/bin`.
    - **Use Shell or Editor** The shell or editor that installed in a `pixi` environment will inherit the `PATH` that effected by `pixi`.
      Calling another executables in the same environment from the shell or editor, like the example above, tends to cause unexpected
      behavior.

--8<-- "docs/partials/conda-style-activation.md"


## Structure

All Pixi environments are by default located in the `.pixi/envs` directory of the workspace.
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
If this is not the case then all the commands that use the environment will automatically it, e.g. `pixi run`, `pixi shell`.

### Environment Metadata
On environment creation, Pixi will add a small file containing some metadata.
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
By default, all lock file modifying commands will always trigger a revalidation, as does `pixi install`.

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

When you run a command that uses the environment, Pixi will check if it is in sync with the `pixi.lock` file.
If it is not, Pixi will solve the environment and update it.
This means that Pixi will retrieve the best set of packages for the dependency requirements that you
specified in the `pixi.toml` and will put the output of the solve step into the `pixi.lock` file.
Solving is a mathematical problem and can take some time, but we take pride in the way we solve
environments, and we are confident that we can solve yours in a reasonable time.
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
This cache folder is shared between all Pixi workspaces and globally installed tools.

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

### De-duplication

When Pixi installs packages into an environment, it does not copy files from the cache.
Instead, it creates **hard links** (or **reflinks** on file systems that support copy-on-write, such as APFS on macOS and btrfs/XFS on Linux).
This means every environment that uses the same version of a package shares the same on-disk files, so the package is effectively stored only once.

For example, if three workspaces all depend on `numpy 1.26.4`, the individual files of that package exist once in the cache and are linked into each environment.
This can save gigabytes of disk space, especially when you work with many environments or large packages like CUDA toolkits.

!!! note "Hard links vs reflinks"
    - **Hard links** point multiple directory entries to the same data on disk. Modifying one link modifies all of them, but this is not an issue because Pixi environments are meant to be read-only.
    - **Reflinks** (copy-on-write links) behave like instant copies that only allocate new disk space when one side is modified. This makes them safer than hard links: if a process accidentally writes to a file in an environment, only that environment's copy is affected while the cached original and all other environments stay intact. On supported file systems Pixi prefers reflinks for this reason.
    - If neither hard links nor reflinks are available (e.g. when the cache and the workspace are on different mount points), Pixi falls back to copying files.
