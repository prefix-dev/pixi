---
part: pixi/ide_integration
title: JupyterLab Integration
description: Use JupyterLab with pixi environments
---

## Basic usage

Using JupyterLab with pixi is very simple.
You can just create a new pixi project and add the `jupyterlab` package to it.
The full example is provided under the following [Github link](https://github.com/prefix-dev/pixi/tree/main/examples/jupyterlab).

```bash
pixi init
pixi add jupyterlab
```

This will create a new pixi project and add the `jupyterlab` package to it. You can then start JupyterLab using the
following command:

```bash
pixi run jupyter lab
```

If you want to add more "kernels" to JupyterLab, you can simply add them to your current project – as well as any dependencies from the scientific stack you might need.

```bash
pixi add bash_kernel ipywidgets matplotlib numpy pandas  # ...
```

### What kernels are available?

You can easily install more "kernels" for JupyterLab. The `conda-forge` repository has a number of interesting additional kernels - not just Python!

- [**`bash_kernel`**](https://prefix.dev/channels/conda-forge/packages/bash_kernel) A kernel for bash
- [**`xeus-cpp`**](https://prefix.dev/channels/conda-forge/packages/xeus-cpp) A C++ kernel based on the new clang-repl
- [**`xeus-cling`**](https://prefix.dev/channels/conda-forge/packages/xeus-cling) A C++ kernel based on the slightly older Cling
- [**`xeus-lua`**](https://prefix.dev/channels/conda-forge/packages/xeus-lua) A Lua kernel
- [**`xeus-sql`**](https://prefix.dev/channels/conda-forge/packages/xeus-sql) A kernel for SQL
- [**`r-irkernel`**](https://prefix.dev/channels/conda-forge/packages/r-irkernel) An R kernel


## Using JupyterLab with the pixi kernel

<!--
Modifications to the following section are related to the README.md in https://github.com/renan-r-santos/pixi-kernel and
https://github.com/renan-r-santos/pixi-kernel-binder, please keep these two in sync by making a PR in both
-->

You can use JupyterLab with pixi by using the kernel provided by the
[pixi-kernel](https://github.com/renan-r-santos/pixi-kernel) package.

## Configuring JupyterLab

To get started, create a `pixi` project and add `jupyterlab` and `pixi-kernel`.

```bash
pixi init
pixi add jupyterlab pixi-kernel
```

Having installed the dependencies, create a folder for your notebooks and start JupyterLab using the following command:

```bash
mkdir -p pixi-notebooks
pixi run jupyter lab --notebook-dir=pixi-notebooks
```

This will start JupyterLab and open it in your browser.

![JupyterLab launcher screen showing Pixi Kernel](https://raw.githubusercontent.com/renan-r-santos/pixi-kernel/main/assets/launch-light.png#only-light)
![JupyterLab launcher screen showing Pixi Kernel](https://raw.githubusercontent.com/renan-r-santos/pixi-kernel/main/assets/launch-dark.png#only-dark)

## Using Pixi in notebooks

You need to create a `pixi` project specific to the folder where your notebooks are located and add `ipykernel` as a
dependency:

```bash
cd pixi-notebooks
pixi init
pixi add ipykernel
```

Then create a notebook and when asked to select a kernel, choose `Pixi`.

## Binder

If you just want to test using Pixi in JupyterLab, you can go directly to
[Binder](https://mybinder.org/v2/gh/renan-r-santos/pixi-kernel-binder/main?labpath=example.ipynb).

The repository [pixi-kernel-binder](https://github.com/renan-r-santos/pixi-kernel-binder) provides all the configuration
needed to run Pixi on Binder.
