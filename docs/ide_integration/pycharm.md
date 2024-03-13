---
part: pixi/ide_integration
title: PyCharm Integration
description: Use PyCharm with pixi environments
---
<!--
Modifications to this file are related to the README.md in https://github.com/pavelzw/pixi-pycharm,
please keep these two in sync by making a PR in both
-->

You can use PyCharm with pixi environments by using the `conda` shim provided by the [pixi-pycharm](https://github.com/pavelzw/pixi-pycharm) package.

!!!warning "Windows support"
    Windows is currently not supported, see [pavelzw/pixi-pycharm #5](https://github.com/pavelzw/pixi-pycharm/issues/5). Only Linux and macOS are supported.

## How to use

To get started, add `pixi-pycharm` to your pixi project.

```bash
pixi add pixi-pycharm
```

This will ensure that the conda shim is installed in your project's environment.

!!!tip "could not determine any available versions for pixi-pycharm on win-64"
    If you get the error `could not determine any available versions for pixi-pycharm on win-64` when running `pixi add pixi-pycharm` (even when you're not on Windows),
    this is because the package is not available on Windows and pixi tries to solve the environment for all platforms.
    If you still want to use it in your pixi project (and are on Linux/macOS), you can add the following to your `pixi.toml`:

    ```toml
    [target.osx-arm64.dependencies] # (1)!
    pixi-pycharm = "*"
    ```

    1. Or `[target.linux-64.dependencies]` depending on your platform.

    This will tell pixi to only use this dependency on a specific platform.

Having `pixi-pycharm` installed, you can now configure PyCharm to use your pixi environments.
Go to the *Add Python Interpreter* dialog (bottom right corner of the PyCharm window) and select *Conda Environment*.
Set *Conda Executable* to the full path of the `conda` file in your pixi environment.
You can get the path using the following command:

```bash
pixi run 'echo $CONDA_PREFIX/libexec/conda'
```

This is an executable that tricks PyCharm into thinking it's the proper `conda` executable.
Under the hood it redirects all calls to the corresponding `pixi` equivalent.

!!!warning "Use the conda shim from this pixi project"
    Please make sure that this is the `conda` shim from this pixi project and not another one.
    If you use multiple pixi projects, you might have to adjust the path accordingly as PyCharm remembers the path to the conda executable.

![Add Python Interpreter](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/add-conda-environment-light.png#only-light)
![Add Python Interpreter](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/add-conda-environment-dark.png#only-dark)

Having selected the environment, PyCharm will now use the Python interpreter from your pixi environment.

PyCharm should now be able to show you the installed packages as well.

![PyCharm package list](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/dependency-list-light.png#only-light)
![PyCharm package list](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/dependency-list-dark.png#only-dark)

You can now run your programs and tests as usual.

![PyCharm run tests](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/tests-light.png#only-light)
![PyCharm run tests](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/tests-dark.png#only-dark)

### Multiple environments

If your project uses [multiple environments](../environment.md) to tests different Python versions or dependencies, you can add multiple environments to PyCharm
by specifying *Use existing environment* in the *Add Python Interpreter* dialog.

![Multiple pixi environments](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/python-interpreters-multi-env-light.png#only-light)
![Multiple pixi environments](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/python-interpreters-multi-env-dark.png#only-dark)

You can then specify the corresponding environment in the bottom right corner of the PyCharm window.

![Specify environment](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/specify-interpreter-light.png#only-light)
![Specify environment](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/specify-interpreter-dark.png#only-dark)

### Multiple pixi projects

When using multiple pixi projects, remember to select the correct *Conda Executable* for each project as mentioned above.
It also might come up that you have multiple environments it might come up that you have multiple environments with the same name.

![Multiple default environments](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/multiple-default-envs-light.png#only-light)
![Multiple default environments](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/multiple-default-envs-dark.png#only-dark)

It is recommended to rename the environments to something unique.

## Debugging

Logs are written to `~/.cache/pixi-pycharm.log`.
You can use them to debug problems.
Please attach the logs when [filing a bug report](https://github.com/pavelzw/pixi-pycharm/issues/new?template=bug-report.md).
