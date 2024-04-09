---
part: pixi/ide_integration
title: JupyterLab Integration
description: Use JupyterLab with pixi environments
---

<!--
Modifications to this file are related to the README.md in https://github.com/renan-r-santos/pixi-kernel and
https://github.com/renan-r-santos/pixi-kernel-binder, please keep these two in sync by making a PR in both
-->

You can use JupyterLab with pixi by using the kernel provided by the
[pixi-kernel](https://github.com/pavelzw/pixi-pycharm) package.

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
