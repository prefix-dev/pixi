## Overview
This guide explains how to integrate PyTorch with `pixi`, it supports multiple ways of installing PyTorch.

- Install PyTorch using `conda-forge` Conda channel (Recommended)
- Install PyTorch using `pypi`, using our `uv`'s integration. (Most versions available)
- Install PyTorch using `pytorch` Conda channel (Legacy)

With these options you can choose the best way to install PyTorch based on your requirements.

## System requirements
In the context of PyTorch, [**system requirements**](../workspace/system_requirements.md) help Pixi understand whether it can install and use CUDA-related packages.
These requirements ensure compatibility during dependency resolution.

The key mechanism here is the use of virtual packages like __cuda.
Virtual packages signal the available system capabilities (e.g., CUDA version).
By specifying `system-requirements.cuda = "12"`, you are telling Pixi that CUDA version 12 is available and can be used during resolution.

For example:

- If a package depends on `__cuda >= 12`, Pixi will resolve the correct version.
- If a package depends on `__cuda` without version constraints, any available CUDA version can be used.

Without setting the appropriate `system-requirements.cuda`, Pixi will default to installing the **CPU-only** versions of PyTorch and its dependencies.

A more in-depth explanation of system requirements can be found [here](../workspace/system_requirements.md).

## Installing from Conda-forge
You can install PyTorch using the `conda-forge` channel.
These are the conda-forge community maintained builds of PyTorch.
You can make direct use of the Nvidia provided packages to make sure the packages can work together.

=== "`pixi.toml`"
    ```toml title="Bare minimum conda-forge pytorch with cuda installation"
    --8<-- "docs/source_files/pixi_tomls/pytorch-conda-forge.toml:minimal"
    ```
=== "`pyproject.toml`"
    ```toml title="Bare minimum conda-forge pytorch with cuda installation"
    --8<-- "docs/source_files/pyproject_tomls/pytorch-conda-forge.toml:minimal"
    ```

To deliberately install a specific version of the `cuda` packages you can depend on the `cuda-version` package which will then be interpreted by the other packages during resolution.
The `cuda-version` package constraints the version of the `__cuda` virtual package and `cudatoolkit` package.
This ensures that the correct version of the `cudatoolkit` package is installed and the tree of dependencies is resolved correctly.

=== "`pixi.toml`"
    ```toml title="Add cuda version to the conda-forge pytorch installation"
    --8<-- "docs/source_files/pixi_tomls/pytorch-conda-forge.toml:cuda-version"
    ```
=== "`pyproject.toml`"
    ```toml title="Add cuda version to the conda-forge pytorch installation"
    --8<-- "docs/source_files/pyproject_tomls/pytorch-conda-forge.toml:cuda-version"
    ```

With `conda-forge` you can also install the `cpu` version of PyTorch.
A common use-case is having two environments, one for CUDA machines and one for non-CUDA machines.

=== "`pixi.toml`"
    ```toml title="Adding a cpu environment"
    --8<-- "docs/source_files/pixi_tomls/pytorch-conda-forge-envs.toml:use-envs"
    ```
=== "`pyproject.toml`"
    ```toml title="Split into environments and add a CPU environment"
    --8<-- "docs/source_files/pyproject_tomls/pytorch-conda-forge-envs.toml:use-envs"
    ```

Running these environments then can be done with the `pixi run` command.
```shell
pixi run --environment cpu python -c "import torch; print(torch.cuda.is_available())"
pixi run -e gpu python -c "import torch; print(torch.cuda.is_available())"
```

Now you should be able to extend that with your dependencies and tasks.

Here are some links to notable packages:

- [pytorch](https://prefix.dev/channels/conda-forge/packages/pytorch)
- [pytorch-cpu](https://prefix.dev/channels/conda-forge/packages/pytorch-cpu)
- [pytorch-gpu](https://prefix.dev/channels/conda-forge/packages/pytorch-gpu)
- [torchvision](https://prefix.dev/channels/conda-forge/packages/torchvision)
- [torchaudio](https://prefix.dev/channels/conda-forge/packages/torchaudio)
- [cuda-version](https://prefix.dev/channels/conda-forge/packages/cuda-version)

## Installing from PyPi
Thanks to the integration with `uv` we can also install PyTorch from `pypi`.

!!! note "Mixing `[dependencies]` and `[pypi-dependencies]`"
    When using this approach for the `torch` package, you should also install the packages that depend on `torch` from `pypi`.
    Thus, not mix the PyPI packages with Conda packages if there are dependencies from the Conda packages to the PyPI ones.

    The reason for this is that our resolving is a two step process, first resolve the Conda packages and then the PyPI packages.
    Thus this can not succeed if we require a Conda package to depend on a PyPI package.

### Pytorch index
PyTorch packages are provided through a custom index, these are similar to Conda channels, which are maintained by the PyTorch team.
To install PyTorch from the PyTorch index, you need to add the indexes to manifest.
Best to do this per dependency to force the index to be used.

- CPU only: [https://download.pytorch.org/whl/cpu](https://download.pytorch.org/whl/cpu)
- CUDA 11.8: [https://download.pytorch.org/whl/cu118](https://download.pytorch.org/whl/cu118)
- CUDA 12.1: [https://download.pytorch.org/whl/cu121](https://download.pytorch.org/whl/cu121)
- CUDA 12.4: [https://download.pytorch.org/whl/cu124](https://download.pytorch.org/whl/cu124)
- ROCm6: [https://download.pytorch.org/whl/rocm6.2](https://download.pytorch.org/whl/rocm6.2)

=== "`pixi.toml`"
    ```toml title="Install PyTorch from pypi"
    --8<-- "docs/source_files/pixi_tomls/pytorch-pypi.toml:minimal"
    ```
=== "`pyproject.toml`"
    ```toml title="Install PyTorch from pypi"
    --8<-- "docs/source_files/pyproject_tomls/pytorch-pypi.toml:minimal"
    ```

You can tell Pixi to use multiple environment for the multiple versions of PyTorch, either `cpu` or `gpu`.

=== "`pixi.toml`"
    ```toml title="Use multiple environments for the pypi pytorch installation"
    --8<-- "docs/source_files/pixi_tomls/pytorch-pypi-envs.toml:multi-env"
    ```
=== "`pyproject.toml`"
    ```toml title="Use multiple environments for the pypi pytorch installation"
    --8<-- "docs/source_files/pyproject_tomls/pytorch-pypi-envs.toml:multi-env"
    ```

Running these environments then can be done with the `pixi run` command.
```shell
pixi run --environment cpu python -c "import torch; print(torch.__version__); print(torch.cuda.is_available())"
pixi run -e gpu python -c "import torch; print(torch.__version__); print(torch.cuda.is_available())"
```

### Mixing MacOS and CUDA with `pypi-dependencies`
When using pypi-dependencies, Pixi creates a “solve” environment to resolve the PyPI dependencies.
This process involves installing the Conda dependencies first and then resolving the PyPI packages within that environment.

This can become problematic if you’re on a macOS machine and trying to resolve the CUDA version of PyTorch for Linux or Windows.
Since macOS doesn’t support those environments, the Conda dependencies for CUDA will fail to install, preventing proper resolution.

**Current Status:**
The Pixi maintainers are aware of this limitation and are actively working on a solution to enable cross-platform dependency resolution for such cases.

In the meantime, you may need to run the resolution process on a machine that supports CUDA, such as a Linux or Windows host.

## Installing from PyTorch channel
!!! warning
    This depends on the [non-free](https://www.anaconda.com/blog/is-conda-free) `main` channel from Anaconda and mixing it with `conda-forge` can lead to conflicts.

!!! note
    This is the [legacy](https://dev-discuss.pytorch.org/t/pytorch-deprecation-of-conda-nightly-builds/2590) way of installing pytorch, this will not be updated to later versions as pytorch has discontinued their channel.

=== "`pixi.toml`"
    ```toml title="Install PyTorch from the PyTorch channel"
    --8<-- "docs/source_files/pixi_tomls/pytorch-from-pytorch-channel.toml:minimal"
    ```
=== "`pyproject.toml`"
    ```toml title="Install PyTorch from the PyTorch channel"
    --8<-- "docs/source_files/pyproject_tomls/pytorch-from-pytorch-channel.toml:minimal"
    ```

## Troubleshooting
When you had trouble figuring out why your PyTorch installation is not working, please share your solution or tips with the community by creating a **PR** to this documentation.

#### Testing the `pytorch` installation
You can verify your PyTorch installation with this command:
```shell
pixi run python -c "import torch; print(torch.__version__); print(torch.cuda.is_available())"
```

#### Checking the CUDA version of your machine
To check which CUDA version Pixi detects on your machine, run:
```
pixi info
```

Example output:
```
...
Virtual packages: __unix=0=0
                : __linux=6.5.9=0
                : __cuda=12.5=0
...
```

If `__cuda` is missing, you can verify your system’s CUDA version using NVIDIA tools:
```shell
nvidia-smi
```

To check the version of the CUDA toolkit installed in your environment:
```shell
pixi run nvcc --version
```

#### Reasons for broken installations
Broken installations often result from mixing incompatible channels or package sources:

1. **Mixing Conda Channels**

    Using both `conda-forge` and the legacy `pytorch` channel can cause conflicts.
    Choose one channel and stick with it to avoid issues in the installed environment.

2. **Mixing Conda and PyPI Packages**

    If you install PyTorch from pypi, all packages that depend on torch must also come from PyPI.
    Mixing Conda and PyPI packages within the same dependency chain leads to conflicts.

To summarize:

- Pick one Conda channel (conda-forge or pytorch) to fetch `pytorch` from, and avoid mixing.
- For PyPI installations, ensure all related packages come from PyPI.

#### GPU version of `pytorch` not installing:

1. Using [conda-Forge](#installing-from-conda-forge)
   - Ensure `system-requirements.cuda` is set to inform Pixi to install CUDA-enabled packages.
   - Use the `cuda-version` package to pin the desired CUDA version.
2. Using [PyPI](#installing-from-pypi)
   - Use the appropriate PyPI index to fetch the correct CUDA-enabled wheels.

#### Environment Resolution Failures
If you see an error like this:
**ABI tag mismatch**
```
├─▶ failed to resolve pypi dependencies
╰─▶ Because only the following versions of torch are available:
      torch<=2.5.1
      torch==2.5.1+cpu
  and torch==2.5.1 has no wheels with a matching Python ABI tag, we can conclude that torch>=2.5.1,<2.5.1+cpu cannot be used.
  And because torch==2.5.1+cpu has no wheels with a matching platform tag and you require torch>=2.5.1, we can conclude that your requirements are
  unsatisfiable.
```
This happens when the Python ABI tag (Application Binary Interface) doesn’t match the available PyPI wheels.

Solution:

- Check your Python version and ensure it’s compatible with the PyPI wheels for `torch`.
The ABI tag is based on the Python version is embedded in the wheel filename, e.g. `cp312` for Python 3.12.
- If needed, lower the `requires-python` or `python` version in your configuration.
- For example, as of now, PyTorch doesn’t fully support Python 3.13; use Python 3.12 or earlier.


**Platform tag mismatch**
```
├─▶ failed to resolve pypi dependencies
╰─▶ Because only the following versions of torch are available:
    torch<=2.5.1
    torch==2.5.1+cu124
and torch>=2.5.1 has no wheels with a matching platform tag, we can conclude that torch>=2.5.1,<2.5.1+cu124 cannot be used.
And because you require torch>=2.5.1, we can conclude that your requirements are unsatisfiable.
```
This occurs when the platform tag doesn’t match the PyPI wheels available to be installed.

Example Issue:
`torch==2.5.1+cu124` (CUDA 12.4) was attempted on an `osx` machine, but this version is only available for `linux-64` and `win-64`.

Solution:
- Use the correct PyPI index for your platform:
  - CPU-only: Use the cpu index for all platforms.
  - CUDA versions: Use cu124 for linux-64 and win-64.

Correct Indexes:
- CPU: https://download.pytorch.org/whl/cpu
- CUDA 12.4: https://download.pytorch.org/whl/cu124

This ensures PyTorch installations are compatible with your system’s platform and Python version.
