# Pytorch Integration Guide

## Overview
This guide explains how to integrate PyTorch with `pixi`, it supports multiple ways of installing PyTorch.

- Install PyTorch using `conda-forge` Conda channel (Recommended)
- Install PyTorch using `pypi`, using our `uv`'s integration. (Most versions available)
- Install PyTorch using `pytorch` Conda channel (Legacy)

With these options you can choose the best way to install PyTorch based on your requirements.

## Installing from Conda-forge
You can install PyTorch using the `conda-forge` channel.
These are the community maintained builds of PyTorch.
You can make direct use of the Nvidia provided packages to make sure the packages can work together.

!!! note "Windows"
    Currently not well-supported for Windows, but there is lots of work being done to get this working.
    Follow the work on the [feedstock](https://github.com/conda-forge/pytorch-cpu-feedstock)
!!! note "System requirements"
    Pixi uses the `system-requirements.cuda` to tell it can use the `cuda` packages.
    Without it, pixi will install the `cpu` versions.
    More information on how to use `system-requirements` can be found [here](./system_requirements.md).

=== "`pixi.toml`"
    ```toml title="Bare minimum conda-forge pytorch with cuda installation"
    --8<-- "docs/source_files/pixi_tomls/pytorch-conda-forge.toml:minimal"
    ```
=== "`pyproject.toml`"
    ```toml title="Bare minimum conda-forge pytorch with cuda installation"
    --8<-- "docs/source_files/pyproject_tomls/pytorch-conda-forge.toml:minimal"
    ```

To deliberately install a specific version of the `cuda` packages you can depend on the `cuda-version` package which will then be interpreted by the other packages during resolution.
The `cuda-version` package constraints the version of other `cuda` packages, like `cudatoolkit` and it is depended on by some package to make sure the correct version is installed.


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
    --8<-- "docs/source_files/pixi_tomls/pytorch-conda-forge.toml:use-envs"
    ```
=== "`pyproject.toml`"
    ```toml title="Split into environments and add a CPU environment"
    --8<-- "docs/source_files/pyproject_tomls/pytorch-conda-forge.toml:use-envs"
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

In the PyPI world there is no such thing as `system-requirements` so you will have to specify this logic yourself.
Otherwise, like in the previous example it will always install the cuda version.
You can tell pixi to use multiple environment for the multiple versions of PyTorch, either `cpu` or `gpu`.

=== "`pixi.toml`"
    ```toml title="Use multiple environments for the pypi pytorch installation"
    --8<-- "docs/source_files/pixi_tomls/pytorch-pypi.toml:multi-env"
    ```
=== "`pyproject.toml`"
    ```toml title="Use multiple environments for the pypi pytorch installation"
    --8<-- "docs/source_files/pyproject_tomls/pytorch-pypi.toml:multi-env"
    ```

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

- You can test the installation with the following command:
```shell
pixi run python -c "import torch; print(torch.__version__); print(torch.cuda.is_available())"
```
- You can ask pixi which version of CUDA it finds on your computer with `pixi info`.
```
> pixi info
...
Virtual packages: __unix=0=0
                : __linux=6.5.9=0
                : __cuda=12.5=0
...
```
- Installing `torch` from PyPI and other packages from Conda channels doesn't work.
  - The lowest level package in the dependency tree that uses a PyPI package demands that all later packages are also PyPI packages.
- Reasons for broken installations
  - Using both `conda-forge` and `pytorch` channels, this can lead to conflicts. Choose one or the other.
- Not installing the GPU version of the `pytorch` package:
  - Using [conda-forge](./#installing-from-conda-forge): Use the `system-requirements.cuda` to tell pixi to install the `cuda` packages. And set the `cuda-version` package to the version you want to install.
  - Using [PyPI](./#installing-from-pypi): Make sure you are using the correct [index](./#pytorch-index) for the version you want to install.
- Not being able to solve the environment:
```
├─▶ failed to resolve pypi dependencies
╰─▶ Because only the following versions of torch are available:
      torch<=2.5.1
      torch==2.5.1+cpu
  and torch==2.5.1 has no wheels with a matching Python ABI tag, we can conclude that torch>=2.5.1,<2.5.1+cpu cannot be used.
  And because torch==2.5.1+cpu has no wheels with a matching platform tag and you require torch>=2.5.1, we can conclude that your requirements are
  unsatisfiable.
```
This error occurs when the ABI tag of the Python version doesn't match the wheels available on PyPI.
Fix this by lowering the `requires-python` or `python` dependency.
At the time of writing Python 3.13 is not supported by many PyTorch-dependent wheels yet.
