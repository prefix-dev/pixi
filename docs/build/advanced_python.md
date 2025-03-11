# Tutorial: Building a Python Package with rattler-build and recipe.yaml

In this tutorial, we will show you how to build the same Python package as from [Building a Python Package](python.md) tutorial using a `recipe.yaml` with `rattler-build`.

This approach may be useful when no build backend for your language or build system exists. You may use it for example when building a package that would require a `go` backend.


Another reason to use it when you would like to have more control over the build process, by passing custom flags to the build system, pre or post-process the build artifacts, which is not possible with the existing backends.


To illustrate this, we will use the same Python package as in the previous tutorial, but this time we will use `rattler-build` to build it. This will unvail the hidden complexity of the build process, and give you a better grasp of how backends work.


!!! warning
    `pixi-build` is a preview feature, and will change until it is stabilized.
    Please keep that in mind when you use it for your workspaces.

!!! hint
    Prefer using a backend if it exists. This will give you a more streamlined and unified build experience.

## Let's get started

First, we create a simple Python package with a `pyproject.toml` and a single Python file.
The package will be called `rich_example`, so we will create the following structure:

```shell
├── src # (1)!
│   └── rich_example
│       └── __init__.py
└── pyproject.toml
```

1. This project uses a src-layout, but Pixi supports both [flat- and src-layouts](https://packaging.python.org/en/latest/discussions/src-layout-vs-flat-layout/#src-layout-vs-flat-layout).


The Python package has a single function `main`.
Calling that, will print a table containing the name, age and city of three people.

```py title="src/rich_example/__init__.py"
--8<-- "docs/source_files/pixi_workspaces/pixi_build/advanced_python/src/rich_example/__init__.py"
```


The metadata of the Python package is defined in `pyproject.toml`.

```toml title="pyproject.toml"
--8<-- "docs/source_files/pixi_workspaces/pixi_build/advanced_python/pyproject.toml"
```

1. We use the `rich` package to print the table in the terminal.
2. By specifying a script, the executable `rich-example-main` will be available in the environment. When being called it will in return call the `main` function of the `rich_example` module.
3. One can choose multiple backends to build a Python package, we choose `hatchling` which works well without additional configuration.


### Adding a `pixi.toml`

What we have in the moment, constitutes a full Python package.
It could be uploaded to [PyPI](https://pypi.org/) as-is.

However, we still need a tool to manage our environments and if we want other Pixi projects to depend on our tool, we need to include more information.
We will do exactly that by creating a `pixi.toml`.

!!! note
    The Pixi manifest can be in its own `pixi.toml` file or integrated in `pyproject.toml`
    In this tutorial, we will use `pixi.toml`.
    If you want everything integrated in `pyproject.toml` just copy the content of `pixi.toml` in this tutorial to your `pyproject.toml` and prepend `tool.pixi.` to each table.

Let's initialize a Pixi project.

```
pixi init --format pixi
```

We pass `--format pixi` in order to communicate to pixi, that we want a `pixi.toml` rather than extending `pyproject.toml`.


```shell
├── src
│   └── rich_example
│       └── __init__.py
├── .gitignore
├── pixi.toml
└── pyproject.toml
```


## The `recipe.yaml` file

Next lets add the `recipe.yaml` file that will be used to build the package:

```yaml
--8<-- "docs/source_files/pixi_workspaces/pixi_build/advanced_python/recipe.yaml"
```

1. Because we are specifying current directory as the source directory, `rattler-build` may skip files that are not tracked by git. We need to ignore `gitignore`.  If your files are already tracked by git, you can remove this line.
2. When building, we want to invoke the python's build frontend `pip` which will invoke `hatchling` backend to build the package.
3. For host dependencies we `python`, `pip` and `hatchling`.


This is the content of the `pixi.toml`:

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/pixi_build/advanced_python/pixi.toml"
```

1. In `workspace` information is set that is shared across all packages in the workspace.
2. In `dependencies` you specify all of your Pixi packages. Here, this includes only our own package that is defined further below under `package`
3. We define a task that runs the `rich-example-main` executable we defined earlier. You can learn more about tasks in this [section](../environments/advanced_tasks.md)
4. In `package` we define the actual Pixi package. This information will be used when other Pixi packages or workspaces depend on our package or when we upload it to a conda channel.
5. We will use `pixi-build-rattler-build` to build the python package using `recipe.yaml`.


When we now run `pixi run start`, we get the following output:

```
┏━━━━━━━━━━━━━━┳━━━━━┳━━━━━━━━━━━━━┓
┃ name         ┃ age ┃ city        ┃
┡━━━━━━━━━━━━━━╇━━━━━╇━━━━━━━━━━━━━┩
│ John Doe     │ 30  │ New York    │
│ Jane Smith   │ 25  │ Los Angeles │
│ Tim de Jager │ 35  │ Utrecht     │
└──────────────┴─────┴─────────────┘
```

## Conclusion

In this tutorial, we created a Pixi package using `rattler-build` and a `recipe.yaml` file.
Using this approach, you have more control over the build process.
For example, you could using `build` python frontend instead of `pip` or you could create dynamic `setup.py` file that would be used to build the package.

At the same time, you are losing all the benefit of heavy lifting that is done by language build backend.

Thanks for reading! Happy Coding 🚀

Any questions? Feel free to reach out or share this tutorial on [X](https://twitter.com/prefix_dev), [join our Discord](https://discord.gg/kKV8ZxyzY4), [e-mail](mailto:hi@prefix.dev) us or follow our [GitHub](https://github.com/prefix-dev).
