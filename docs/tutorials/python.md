# Tutorial: Doing Python development with Pixi

In this tutorial, we will show you how to create a simple Python project with pixi.
We will show some of the features that pixi provides, that are currently not a part of `pdm`, `poetry` etc.

## Why is this useful?

Pixi builds upon the conda ecosystem, which allows you to create a Python environment with all the dependencies you need.
This is especially useful when you are working with multiple Python interpreters and bindings to C and C++ libraries.
For example, GDAL from PyPI does not have binary C dependencies, but the conda package does.
On the other hand, some packages are only available through PyPI, which `pixi` can also install for you.
Best of both world, let's give it a go!

## `pixi.toml` and `pyproject.toml`

We support two manifest formats: `pyproject.toml` and `pixi.toml`.
In this tutorial, we will use the `pyproject.toml` format because it is the most common format for Python projects.

## Let's get started

Let's start out by making a directory and creating a new `pyproject.toml` file.

```shell
pixi init pixi-py --format pyproject
```

This gives you the following pyproject.toml:

```toml
[project]
name = "pixi-py"
version = "0.1.0"
description = "Add a short description here"
authors = [{name = "Tim de Jager", email = "tim@prefix.dev"}]
requires-python = ">= 3.11"
dependencies = []

[build-system]
build-backend = "hatchling.build"
requires = ["hatchling"]

[tool.pixi.project]
channels = ["conda-forge"]
platforms = ["osx-arm64"]

[tool.pixi.pypi-dependencies]
pixi-py = { path = ".", editable = true }

[tool.pixi.tasks]
```

Let's add the Python project to the tree:

=== "Linux & macOS"
    ```shell
    cd pixi-py # move into the project directory
    mkdir pixi_py
    touch pixi_py/__init__.py
    ```

=== "Windows"
    ```shell
    cd pixi-py
    mkdir pixi_py
    type nul > pixi_py\__init__.py
    ```

We now have the following directory structure:

```shell
.
├── pixi_py
│   └── __init__.py
└── pyproject.toml
```

We've used a flat-layout here but pixi supports both [flat- and src-layouts](https://packaging.python.org/en/latest/discussions/src-layout-vs-flat-layout/#src-layout-vs-flat-layout).

### What's in the `pyproject.toml`?

Okay, so let's have a look at what sections have been added and how we can modify the `pyproject.toml`.

These first entries were added to the `pyproject.toml` file:

```toml
# Main pixi entry
[tool.pixi.project]
channels = ["conda-forge"]
# This is your machine platform by default
platforms = ["osx-arm64"]
```

The `channels` and `platforms` are added to the `[tool.pixi.project]` section.
Channels like `conda-forge` manage packages similar to PyPI but allow for different packages across languages.
The keyword `platforms` determines what platform the project supports.

The `pixi_py` package itself is added as an editable dependency.
This means that the package is installed in editable mode, so you can make changes to the package and see the changes reflected in the environment, without having to re-install the environment.

```toml
# Editable installs
[tool.pixi.pypi-dependencies]
pixi-py = { path = ".", editable = true }
```

In pixi, unlike other package managers, this is explicitly stated in the `pyproject.toml` file.
The main reason being so that you can choose which environment this package should be included in.

### Managing both conda and PyPI dependencies in pixi

Our projects usually depend on other packages.

```shell
$ pixi add black
Added black
```

This will result in the following addition to the `pyproject.toml`:

```toml
# Dependencies
[tool.pixi.dependencies]
black = ">=24.4.2,<24.5"
```

But we can also be strict about the version that should be used with `pixi add black=24`, resulting in

```toml
[tool.pixi.dependencies]
black = "24.*"
```

Now, let's add some optional dependencies:

```shell
pixi add --pypi --feature test pytest
```

Which results in the following fields added to the `pyproject.toml`:
```toml
[project.optional-dependencies]
test = ["pytest"]
```

After we have added the optional dependencies to the `pyproject.toml`, pixi automatically creates a [`feature`](../reference/project_configuration.md/#the-feature-and-environments-tables), which can contain a collection of `dependencies`, `tasks`, `channels`, and more.

Sometimes there are packages that aren't available on conda channels but are published on PyPI.
We can add these as well, which pixi will solve together with the default dependencies.

```shell
$ pixi add black --pypi
Added black
Added these as pypi-dependencies.
```

which results in the addition to the `dependencies` key in the `pyproject.toml`

```toml
dependencies = ["black"]
```

When using the `pypi-dependencies` you can make use of the `optional-dependencies` that other packages make available.
For example, `black` makes the `cli` dependencies option, which can be added with the `--pypi` keyword:

```shell
$ pixi add black[cli] --pypi
Added black[cli]
Added these as pypi-dependencies.
```

which updates the `dependencies` entry to

```toml
dependencies = ["black[cli]"]
```

??? note "Optional dependencies in `pixi.toml`"
    This tutorial focuses on the use of the `pyproject.toml`, but in case you're curious, the `pixi.toml` would contain the following entry after the installation of a PyPI package including an optional dependency:
    ```toml
    [pypi-dependencies]
    black = { version = "*", extras = ["cli"] }
    ```


### Installation: `pixi install`

Now let's `install` the project with `pixi install`:

```shell
$ pixi install
✔ Project in /path/to/pixi-py is ready to use!
```

We now have a new directory called `.pixi` in the project root.
This directory contains the environment that was created when we ran `pixi install`.
The environment is a conda environment that contains the dependencies that we specified in the `pyproject.toml` file.
We can also install the test environment with `pixi install -e test`.
We can use these environments for executing code.

We also have a new file called `pixi.lock` in the project root.
This file contains the exact versions of the dependencies that were installed in the environment across platforms.

## What's in the environment?

Using `pixi list`, you can see what's in the environment, this is essentially a nicer view on the lock file:

```shell
$ pixi list
Package          Version       Build               Size       Kind   Source
bzip2            1.0.8         h93a5062_5          119.5 KiB  conda  bzip2-1.0.8-h93a5062_5.conda
black            24.4.2                            3.8 MiB    pypi   black-24.4.2-cp312-cp312-win_amd64.http.whl
ca-certificates  2024.2.2      hf0a4a13_0          152.1 KiB  conda  ca-certificates-2024.2.2-hf0a4a13_0.conda
libexpat         2.6.2         hebf3989_0          62.2 KiB   conda  libexpat-2.6.2-hebf3989_0.conda
libffi           3.4.2         h3422bc3_5          38.1 KiB   conda  libffi-3.4.2-h3422bc3_5.tar.bz2
libsqlite        3.45.2        h091b4b1_0          806 KiB    conda  libsqlite-3.45.2-h091b4b1_0.conda
libzlib          1.2.13        h53f4e23_5          47 KiB     conda  libzlib-1.2.13-h53f4e23_5.conda
ncurses          6.4.20240210  h078ce10_0          801 KiB    conda  ncurses-6.4.20240210-h078ce10_0.conda
openssl          3.2.1         h0d3ecfb_1          2.7 MiB    conda  openssl-3.2.1-h0d3ecfb_1.conda
python           3.12.3        h4a7b5fc_0_cpython  12.6 MiB   conda  python-3.12.3-h4a7b5fc_0_cpython.conda
readline         8.2           h92ec313_1          244.5 KiB  conda  readline-8.2-h92ec313_1.conda
tk               8.6.13        h5083fa2_1          3 MiB      conda  tk-8.6.13-h5083fa2_1.conda
tzdata           2024a         h0c530f3_0          117 KiB    conda  tzdata-2024a-h0c530f3_0.conda
pixi-py          0.1.0                                        pypi   . (editable)
xz               5.2.6         h57fd34a_0          230.2 KiB  conda  xz-5.2.6-h57fd34a_0.tar.bz2
```

!!! Python interpreters
    The Python interpreter is also installed in the environment.
    This is because the Python interpreter version is read from the `requires-python` field in the `pyproject.toml` file.
    This is used to determine the Python version to install in the environment.
    This way, pixi automatically manages/bootstraps the Python interpreter for you, so no more `brew`, `apt` or other system install steps.

Here, you can see the different conda and Pypi packages listed.
As you can see, the `pixi-py` package that we are working on is installed in editable mode.
Every environment in pixi is isolated but reuses files that are hard-linked from a central cache directory.
This means that you can have multiple environments with the same packages but only have the individual files stored once on disk.

We can create the `default` and `test` environments based on our own `test` feature from the `optional-dependency`:

```shell
pixi project environment add default --solve-group default
pixi project environment add test --feature test --solve-group default
```

Which results in:

```toml
# Environments
[tool.pixi.environments]
default = { solve-group = "default" }
test = { features = ["test"], solve-group = "default" }
```

??? note "Solve Groups"
    Solve groups are a way to group dependencies together.
    This is useful when you have multiple environments that share the same dependencies.
    For example, maybe `pytest` is a dependency that influences the dependencies of the `default` environment.
    By putting these in the same solve group, you ensure that the versions in `test` and `default` are exactly the same.

The `default` environment is created when you run `pixi install`.
The `test` environment is created from the optional dependencies in the `pyproject.toml` file.
You can execute commands in this environment with e.g. `pixi run -e test python`

## Getting code to run

Let's add some code to the `pixi-py` package.
We will add a new function to the `pixi_py/__init__.py` file:

```python
from rich import print

def hello():
    return "Hello, [bold magenta]World[/bold magenta]!", ":vampire:"

def say_hello():
    print(*hello())
```

Now add the `rich` dependency from PyPI using: `pixi add --pypi rich`.

Let's see if this works by running:

```shell
pixi r python -c "import pixi_py; pixi_py.say_hello()"
Hello, World! 🧛
```

??? note "Slow?"
    This might be slow(2 minutes) the first time because pixi installs the project, but it will be near instant the second time.

Pixi runs the self installed Python interpreter.
Then, we are importing the `pixi_py` package, which is installed in editable mode.
The code calls the `say_hello` function that we just added.
And it works! Cool!

## Testing this code

Okay, so let's add a test for this function.
Let's add a `tests/test_me.py` file in the root of the project.

Giving us the following project structure:

```shell
.
├── pixi.lock
├── pixi_py
│   └── __init__.py
├── pyproject.toml
└── tests/test_me.py
```

```python
from pixi_py import hello

def test_pixi_py():
    assert hello() == ("Hello, [bold magenta]World[/bold magenta]!", ":vampire:")
```

Let's add an easy task for running the tests.

```shell
$ pixi task add --feature test test "pytest"
✔ Added task `test`: pytest .
```

So pixi has a task system to make it easy to run commands.
Similar to `npm` scripts or something you would specify in a `Justfile`.

??? tip "Pixi tasks"
    Tasks are actually a pretty cool pixi feature that is powerful and runs in a cross-platform shell.
    You can do caching, dependencies and more.
    Read more about tasks in the [tasks](../features/advanced_tasks.md) section.

```shell
$ pixi r test
✨ Pixi task (test): pytest .
================================================================================================= test session starts =================================================================================================
platform darwin -- Python 3.12.2, pytest-8.1.1, pluggy-1.4.0
rootdir: /private/tmp/pixi-py
configfile: pyproject.toml
collected 1 item

test_me.py .                                                                                                                                                                                                    [100%]

================================================================================================== 1 passed in 0.00s =================================================================================================
```

Neat! It seems to be working!

### Test vs Default environment

Let's compare the output of the test and default environments...

```shell
pixi list -e test
# vs. default environment
pixi list
```

We see that the test environment has:

```shell
package          version       build               size       kind   source
...
pytest           8.1.1                             1.1 mib    pypi   pytest-8.1.1-py3-none-any.whl
...
```

However, the default environment is missing this package.
This way, you can finetune your environments to only have the packages that are needed for that environment.
E.g. you could also have a `dev` environment that has `pytest` and `ruff` installed, but you could omit these from the `prod` environment.
There is a [docker](https://github.com/prefix-dev/pixi/tree/main/examples/docker) example that shows how to set up a minimal `prod` environment and copy from there.

## Replacing PyPI packages with conda packages

Last thing, pixi provides the ability for `pypi` packages to depend on `conda` packages.
Let's confirm this with `pixi list`:

```shell
$ pixi list
Package          Version       Build               Size       Kind   Source
...
pygments         2.17.2                            4.1 MiB    pypi   pygments-2.17.2-py3-none-any.http.whl
...
```

Let's explicitly add `pygments` to the `pyproject.toml` file.
Which is a dependency of the `rich` package.

```shell
pixi add pygments
```

This will add the following to the `pyproject.toml` file:

```toml
[tool.pixi.dependencies]
pygments = ">=2.17.2,<2.18"
```

We can now see that the `pygments` package is now installed as a conda package.

```shell
$ pixi list
Package          Version       Build               Size       Kind   Source
...
pygments         2.17.2        pyhd8ed1ab_0        840.3 KiB  conda  pygments-2.17.2-pyhd8ed1ab_0.conda
```

This way, PyPI dependencies and conda dependencies can be mixed and matched to seamlessly interoperate.

```shell
$  pixi r python -c "import pixi_py; pixi_py.say_hello()"
Hello, World! 🧛
```

And it still works!

## Conclusion

In this tutorial, you've seen how easy it is to use a `pyproject.toml` to manage your pixi dependencies and environments.
We have also explored how to use PyPI and conda dependencies seamlessly together in the same project and install optional dependencies to manage Python packages.

Hopefully, this provides a flexible and powerful way to manage your Python projects and a fertile base for further exploration with Pixi.

Thanks for reading! Happy Coding 🚀

Any questions? Feel free to reach out or share this tutorial on [X](https://twitter.com/prefix_dev), [join our Discord](https://discord.gg/kKV8ZxyzY4), send us an [e-mail](mailto:hi@prefix.dev) or follow our [GitHub](https://github.com/prefix-dev).
