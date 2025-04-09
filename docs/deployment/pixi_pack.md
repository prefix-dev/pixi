<!-- Keep in sync with https://github.com/quantco/pixi-pack/blob/main/README.md -->

[`pixi-pack`](https://github.com/quantco/pixi-pack) is a simple tool that takes a Pixi environment and packs it into a compressed archive that can be shipped to the target machine.

It can be installed via

```bash
pixi global install pixi-pack
```

Or by downloading our pre-built binaries from the [releases page](https://github.com/quantco/pixi-pack/releases).

Instead of installing pixi-pack globally, you can also use Pixi exec to run `pixi-pack` in a temporary environment:

```bash
pixi exec pixi-pack pack
pixi exec pixi-pack unpack environment.tar
```

![pixi-pack demo](https://raw.githubusercontent.com/quantco/pixi-pack/refs/heads/main/.github/assets/demo/demo-light.gif#only-light)
![pixi-pack demo](https://raw.githubusercontent.com/quantco/pixi-pack/refs/heads/main/.github/assets/demo/demo-dark.gif#only-dark)

You can pack an environment with

```bash
pixi-pack pack --manifest-file pixi.toml --environment prod --platform linux-64
```

This will create a `environment.tar` file that contains all conda packages required to create the environment.

```plain
# environment.tar
| pixi-pack.json
| environment.yml
| channel
|    ├── noarch
|    |    ├── tzdata-2024a-h0c530f3_0.conda
|    |    ├── ...
|    |    └── repodata.json
|    └── linux-64
|         ├── ca-certificates-2024.2.2-hbcca054_0.conda
|         ├── ...
|         └── repodata.json
```

### Unpacking an Environment

With `pixi-pack unpack environment.tar`, you can unpack the environment on your target system. This will create a new conda environment in `./env` that contains all packages specified in your `pixi.toml`. It also creates an `activate.sh` (or `activate.bat` on Windows) file that lets you activate the environment without needing to have `conda` or `micromamba` installed.

### Cross-platform Packs

Since `pixi-pack` just downloads the `.conda` and `.tar.bz2` files from the conda repositories, you can trivially create packs for different platforms.

```bash
pixi-pack pack --platform win-64
```

!!! note
    You can only unpack a pack on a system that has the same platform as the pack was created for.

### Self-Extracting Binaries

You can create a self-extracting binary that contains the packed environment and a script that unpacks the environment.
This can be useful if you want to distribute the environment to users that don't have `pixi-pack` installed.

=== "Linux & macOS"
    ```bash
    $ pixi-pack pack --create-executable
    $ ls
    environment.sh
    $ ./environment.sh
    $ ls
    env/
    activate.sh
    environment.sh
    ```

=== "Windows"
    ```powershell
    PS > pixi-pack pack --create-executable
    PS > ls
    environment.ps1
    PS > .\environment.ps1
    PS > ls
    env/
    activate.sh
    environment.ps1
    ```

!!! note

    The produced executable is a simple shell script that contains both the `pixi-pack` binary as well as the packed environment.

### Inject Additional Packages

You can inject additional packages into the environment that are not specified in `pixi.lock` by using the `--inject` flag:

```bash
pixi-pack pack --inject local-package-1.0.0-hbefa133_0.conda --manifest-pack pixi.toml
```

This can be particularly useful if you build the package itself and want to include the built package in the environment but still want to use `pixi.lock` from the workspace.

### PyPi support

You can also pack PyPi wheel packages into your environment.
`pixi-pack` only supports wheel packages and not source distributions.
If you happen to use source distributions, you can ignore them by using the `--ignore-pypi-non-wheel` flag.
This will skip the bundling of PyPi packages that are source distributions.

### Cache Downloaded Packages

You can cache downloaded packages to speed up subsequent pack operations by using the `--use-cache` flag:

```bash
pixi-pack pack --use-cache ~/.pixi-pack/cache
```

This will store all downloaded packages in the specified directory and reuse them in future pack operations. The cache follows the same structure as conda channels, organizing packages by platform subdirectories (e.g., linux-64, win-64, etc.).

Using a cache is particularly useful when:

- Creating multiple packs with overlapping dependencies
- Working with large packages that take time to download
- Operating in environments with limited bandwidth
- Running CI/CD pipelines where package caching can significantly improve build times

### Unpacking Without pixi-pack

If you don't have `pixi-pack` available on your target system, and do not want to use self-extracting binaries (see above), you can still install the environment if you have `conda` or `micromamba` available.
Just unarchive the `environment.tar`, then you have a local channel on your system where all necessary packages are available.
Next to this local channel, you will find an `environment.yml` file that contains the environment specification.
You can then install the environment using `conda` or `micromamba`:

```bash
tar -xvf environment.tar
micromamba create -p ./env --file environment.yml
# or
conda env create -p ./env --file environment.yml
```

!!! note

    The `environment.yml` and `repodata.json` files are only for this use case, `pixi-pack unpack` does not use them.

!!! note

    Both `conda` and `mamba` are always installing pip as a side effect when they install python, see [`conda`'s documentation](https://docs.conda.io/projects/conda/en/25.1.x/user-guide/configuration/settings.html#add-pip-as-python-dependency-add-pip-as-python-dependency).
    This is different from how `pixi` works and can lead to solver errors when using `pixi-pack`'s compatibility mode since `pixi-pack` doesn't include `pip` by default.
    You can fix this issue in two ways:

    - Add `pip` to your `pixi.lock` file using `pixi add pip`.
    - Configuring `conda` (or `mamba`) to not install `pip` by default by running `conda config --set add_pip_as_python_dependency false` (or by adding `add_pip_as_python_dependency: False` to your `~/.condarc`)
