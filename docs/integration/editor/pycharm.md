<!-- Keep in sync with https://github.com/pavelzw/pixi-pycharm/blob/main/README.md -->

You can use PyCharm with Pixi environments by using the `conda` shim provided by the [pixi-pycharm](https://github.com/pavelzw/pixi-pycharm) package.

## How to use

To get started, add `pixi-pycharm` to your Pixi workspace.

```bash
pixi add pixi-pycharm
```

This will ensure that the conda shim is installed in your workspace's environment.

Having `pixi-pycharm` installed, you can now configure PyCharm to use your Pixi environments.
Go to the _Add Python Interpreter_ dialog (bottom right corner of the PyCharm window) and select _Conda Environment_.
Set _Conda Executable_ to the full path of the `conda` file (on Windows: `conda.bat`) which is located in `.pixi/envs/default/libexec`.
You can get the path using the following command:

=== "Linux & macOS"
    ```bash
    pixi run 'echo $CONDA_PREFIX/libexec/conda'
    ```

=== "Windows"
    ```bash
    pixi run 'echo $CONDA_PREFIX\\libexec\\conda.bat'
    ```

This is an executable that tricks PyCharm into thinking it's the proper `conda` executable.
Under the hood it redirects all calls to the corresponding `pixi` equivalent.

!!!warning "Use the conda shim from this Pixi workspace"
    Please make sure that this is the `conda` shim from this Pixi workspace and not another one.
    If you use multiple Pixi projects, you might have to adjust the path accordingly as PyCharm remembers the path to the conda executable.

![Add Python Interpreter](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/add-conda-environment-light.png#only-light)
![Add Python Interpreter](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/add-conda-environment-dark.png#only-dark)

Having selected the environment, PyCharm will now use the Python interpreter from your Pixi environment.

PyCharm should now be able to show you the installed packages as well.

![PyCharm package list](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/dependency-list-light.png#only-light)
![PyCharm package list](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/dependency-list-dark.png#only-dark)

You can now run your programs and tests as usual.

![PyCharm run tests](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/tests-light.png#only-light)
![PyCharm run tests](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/tests-dark.png#only-dark)

!!!tip "Mark `.pixi` as excluded"
    In order for PyCharm to not get confused about the `.pixi` directory, please mark it as excluded.

    ![Mark Directory as excluded 1](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/mark-directory-as-excluded-1-light.png#only-light)
    ![Mark Directory as excluded 1](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/mark-directory-as-excluded-1-dark.png#only-dark)
    ![Mark Directory as excluded 2](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/mark-directory-as-excluded-2-light.png#only-light)
    ![Mark Directory as excluded 2](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/mark-directory-as-excluded-2-dark.png#only-dark)

    Also, when using a remote interpreter, you should exclude the `.pixi` directory on the remote machine.
    Instead, you should run `pixi install` on the remote machine and select the conda shim from there.
    ![Deployment exclude from remote machine](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/deployment-exclude-pixi-light.png#only-light)
    ![Deployment exclude from remote machine](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/deployment-exclude-pixi-dark.png#only-dark)

### Multiple environments

If your workspace uses [multiple environments](../../workspace/multi_environment.md) to tests different Python versions or dependencies, you can add multiple environments to PyCharm
by specifying _Use existing environment_ in the _Add Python Interpreter_ dialog.

![Multiple Pixi environments](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/python-interpreters-multi-env-light.png#only-light)
![Multiple Pixi environments](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/python-interpreters-multi-env-dark.png#only-dark)

You can then specify the corresponding environment in the bottom right corner of the PyCharm window.

![Specify environment](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/specify-interpreter-light.png#only-light)
![Specify environment](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/specify-interpreter-dark.png#only-dark)

### Multiple Pixi projects

When using multiple Pixi projects, remember to select the correct _Conda Executable_ for each workspace as mentioned above.
It also might come up that you have multiple environments with the same name.

![Multiple default environments](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/multiple-default-envs-light.png#only-light)
![Multiple default environments](https://raw.githubusercontent.com/pavelzw/pixi-pycharm/main/.github/assets/multiple-default-envs-dark.png#only-dark)

It is recommended to rename the environments to something unique.

## Debugging

Logs are written to `~/.cache/pixi-pycharm.log`.
You can use them to debug problems.
Please attach the logs when [filing a bug report](https://github.com/pavelzw/pixi-pycharm/issues/new?template=bug-report.md).

## Install as an optional dependency

In some cases, you might only want to install `pixi-pycharm` on your local dev-machines but not in production.
To achieve this, we can use [multiple environments](../../workspace/multi_environment.md).

```toml
[workspace]
name = "multi-env"
version = "0.1.0"
requires-python = ">=3.12"
dependencies = ["numpy"]

[tool.pixi.workspace]
channels = ["conda-forge"]
platforms = ["linux-64"]

[tool.pixi.feature.lint.dependencies]
ruff =  "*"

[tool.pixi.feature.dev.dependencies]
pixi-pycharm = "*"

[tool.pixi.environments]
# The production environment is the default feature set.
# Adding a solve group to make sure the same versions are used in the `default` and `prod` environments.
prod = { solve-group = "main" }

# Setup the default environment to include the dev features.
# By using `default` instead of `dev` you'll not have to specify the `--environment` flag when running `pixi run`.
default = { features = ["dev"], solve-group = "main" }

# The lint environment doesn't need the default feature set but only the `lint` feature
# and thus can also be excluded from the solve group.
lint = { features = ["lint"], no-default-feature = true }
```

Now you as a user can run `pixi shell`, which will start the default environment.
In production, you then just run `pixi run -e prod COMMAND`, and the minimal prod environment is installed.

## Activate the `pixi` project environment

An alternative approach is to use [`direnv`](../third_party/direnv.md) via a 
[Jetbrains `direnv` plugin](https://plugins.jetbrains.com/plugin/15285-direnv-integration)
which can be used to activate `pixi`.
