# Making a Pixi workspace

Pixi's biggest strength is its ability to create reproducible, powerful, and flexible workspaces.
Let's go over the common steps to create a simple Pixi workspace.

## Creating a Pixi workspace
To create a new Pixi workspace, you can use the `pixi init` command:

```shell
pixi init my_workspace
```

This command creates a new directory called `my_workspace` with the following structure:

```shell
my_workspace
├── .gitattributes
├── .gitignore
└── pixi.toml
```

The `pixi.toml` file is the manifest of your Pixi workspace.
It contains all the information about your workspace, such as its channels, platforms, dependencies, tasks, and more.

The file created by `pixi init` is a minimal manifest that looks like this:

```toml title="pixi.toml"
[workspace]
authors = ["Jane Doe <jane.doe@example.com>"]
channels = ["conda-forge"]
name = "my_workspace"
platforms = ["osx-arm64"]
version = "0.1.0"

[tasks]

[dependencies]
```

??? tip "Do you want autocompletion of the manifest file?"
    As `pixi.toml` has a JSON schema, it is possible to use IDE’s like VSCode to edit the field with autocompletion.
    Install the [Even Better TOML VSCode extension](https://marketplace.visualstudio.com/items?itemName=tamasfe.even-better-toml) to get the best experience.
    Or use the integrated schema support in PyCharm.

## Managing dependencies
After creating the workspace, you can start adding dependencies.
Pixi uses the `pixi add` command to add dependencies to a workspace.
This command will, by default, add the [**conda**](https://prefix.dev/blog/what-is-a-conda-package) dependency to the `pixi.toml`, solve the dependencies, write the [lock file](./workspace/lockfile.md), and install the package in the environment.
For example, let's add `numpy` and `pytest` to the workspace.

```shell
pixi add numpy pytest
```
This results in these lines being added:

```toml title="pixi.toml"
[dependencies]
numpy = ">=2.2.6,<3"
pytest = ">=8.3.5,<9"
```

You can also specify the version of the dependency you want to add.

```shell
pixi add numpy==2.2.6 pytest==8.3.5
```

### PyPI dependencies
Pixi normally uses `conda` packages for dependencies, but you can also add dependencies from [PyPI](https://pypi.org).
Pixi will make sure it doesn't try to install the same package from both sources, and avoid conflicts between them.

If you want to add them to your workspace you can do that with the `--pypi` flag:

```shell
pixi add --pypi requests
```
This will add the `requests` package from PyPI to the workspace:

```toml title="pixi.toml"
[pypi-dependencies]
requests = ">=2.31.0,<3"
```

To learn more about the differences between `conda` and PyPI, see [our Conda & PyPI concept documentation](./concepts/conda_pypi.md).

## Lock file
Pixi will always create a lock file when the dependencies are solved.
This file will contain all the exact versions of the workspace's dependencies (and their dependencies).
This results in a reproducible environment, which you can share with others, and use for testing and deployment.

The lockfile is called `pixi.lock` and it is created in the root of the workspace.
To learn more about lock files, see [our detailed lock file documentation](./workspace/lockfile.md).

```yaml title="pixi.lock"
version: 6
environments:
  default:
    channels:
    - url: https://prefix.dev/conda-forge/
    indexes:
    - https://pypi.org/simple
    packages:
      osx-arm64:
      - conda: https://prefix.dev/conda-forge/osx-arm64/bzip2-1.0.8-h99b78c6_7.conda
      - pypi: ...
packages:
- conda: https://prefix.dev/conda-forge/osx-arm64/bzip2-1.0.8-h99b78c6_7.conda
  sha256: adfa71f158cbd872a36394c56c3568e6034aa55c623634b37a4836bd036e6b91
  md5: fc6948412dbbbe9a4c9ddbbcfe0a79ab
  depends:
  - __osx >=11.0
  license: bzip2-1.0.6
  license_family: BSD
  size: 122909
  timestamp: 1720974522888
- pypi: ...
```

## Managing tasks
Pixi has a built-in cross-platform task runner which allows you to define tasks in the manifest.
Think of tasks as commands (or chains of commands) which you may want to repeat many times over the course of developing a project (for example, running the tests).

This is a great way to share tasks with others and to ensure that the same tasks are run in the same environment on different machines.
The tasks are defined in the `pixi.toml` file under the `[tasks]` section.

You can add one to your workspace by running the `pixi task add` command.

```shell
pixi task add hello "echo Hello, World!"
```
This will add the following lines to the `pixi.toml` file:

```toml title="pixi.toml"
[tasks]
hello = "echo Hello, World!"
```
You can then run the task using the `pixi run` command:

```shell
pixi run hello
```
This will execute the command `echo Hello, World!` in the workspace's default environment.

??? tip "Do you want to use more powerful features?"
    Tasks can be much more powerful, for example:

    ```toml
    [tasks.name-of-powerful-task]
    cmd = "echo This task can do much more! Like have {{ arguments }} and {{ "minijinja" | capitalize }} templates."

    # List of tasks that must be run before this one.
    depends-on = ["other-task"]

    # Working directory relative to the root of the workspace
    cwd = "current/working/directory"

    # List of arguments for the task
    args = [{ arg = "arguments", default = "default arguments" }]

    # Run the command if the input files have changed
    input = ["src"]
    # Run the command if the output files are missing
    output = ["output.txt"]

    # Set environment variables for the task
    env = { MY_ENV_VAR = "value" }
    ```
    More information about tasks can be found in the [Tasks](./workspace/advanced_tasks.md) section of the documentation.

## Environments
Pixi always creates an environment for your workspace (the "default" environment),
which contains your `dependencies` and in which your tasks are run.
You can also include [multiple environments](./workspace/multi_environment.md) in one workspace.
These environments are [located](./reference/pixi_configuration.md#detached-environments "Find out how to move this location if required") in the `.pixi/envs` directory in the root of your workspace.

Using these environments is as simple as running the `pixi run` or `pixi shell` command.
`pixi run` will execute the remaining input as a command (or a task if the input matches the name of a defined task) in the environment, while `pixi shell` will spawn a new shell session in the environment. Both commands "activate" the environment — learn more at [our environment activation documentation](./workspace/environment.md#activation).

```shell
pixi run python -VV
# or:
pixi shell
python -VV
exit
```
