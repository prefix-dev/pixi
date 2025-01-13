# Tutorial: Building a Python package

In this tutorial, we will show you how to create a simple Python package with pixi.

## Why is this useful?

Pixi builds upon the conda ecosystem, which allows you to create a Python environment with all the dependencies you need.
Unlike PyPI, the conda ecosystem is cross-language and also offers packages written in Rust, R, C, C++ and many other languages.

By building a Python package with pixi, you can:

1. manage Python packages and packages written in other languages in the same workspace
2. build both conda and Python packages with the same tool

In this tutorial we will focus on point 1.

## Let's get started

First, we create a simple Python package with a `pyproject.toml` and a single Python file.
The package will be called `rich_example`, so we will create the following structure:

```shell
├── src # (1)!
│   └── rich_example
│       └── __init__.py
└── pyproject.toml
```

1. This project uses a src-layout, but pixi supports both [flat- and src-layouts](https://packaging.python.org/en/latest/discussions/src-layout-vs-flat-layout/#src-layout-vs-flat-layout).


The Python package has a single function `main`.
Calling that, will print a table containing the name, age and city of three people.

```py title="src/rich_example/__init__.py"
--8<-- "docs/source_files/pixi_projects/pixi_build_python/src/rich_example/__init__.py"
```


The metadata of the Python package is defined in `pyproject.toml`.

```toml title="pyproject.toml"
--8<-- "docs/source_files/pixi_projects/pixi_build_python/pyproject.toml"
```

1. We use the `rich` package to print the table in the terminal.
2. By specifying a script, the executable `rich-example-main` will be available in the environment. When being called it will in return call the `main` function of the `rich_example` module.
3. One can choose multiple backends to build a Python package, we choose `hatchling` which works well without additional configuration.


### Adding a `pixi.toml`

What we have in the moment, constitutes a full Python package.
It could be uploaded to [PyPI](https://pypi.org/) as-is.

However, we still need a tool to manage our environments and if we want other pixi projects to depend on our tool, we need to include more information.
We will do exactly that by creating a `pixi.toml`.

!!! note
    The pixi manifest can be in its own `pixi.toml` file or integrated in `pyproject.toml`
    In this tutorial, we will use `pixi.toml`.
    If you want everything integrated in `pyproject.toml` just copy the content of `pixi.toml` in this tutorial to your `pyproject.toml` and append `tool.pixi` to each table.

Let's initialize a pixi project.

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

This is the content of the `pixi.toml`:

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_projects/pixi_build_python/pixi.toml"
```

1. In `workspace` information is set that is shared across all packages in the workspace.
2. In `dependencies` you specify all of your pixi packages. Here, this includes only our own package that is defined further below under `package`
3. We define a task that runs the `rich-example-main` executable we defined earlier. You can learn more about tasks in this [section](../features/advanced_tasks.md)
4. In `package` we define the actual pixi package. This information will be used when other pixi packages or workspaces depend on our package or when we upload it to a conda channel.
5. The same way, Python uses build backends to build a Python package, pixi uses build backends to build pixi packages. `pixi-build-python` creates a pixi package out of a Python package.
6. In `package.host-dependencies`, we add Python dependencies that are necessary to build the Python package. By adding them here as well, the dependencies will come from the conda channel rather than PyPI.
7. In `package.run-dependencies`, we add the Python dependencies needed during runtime.


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

In this tutorial, we created a pixi package based on Python.
It can be used as-is, to upload to a conda channel or to PyPI.
In another tutorial we will learn how to add multiple pixi packages to the same workspace and let one pixi package use another.

Thanks for reading! Happy Coding 🚀

Any questions? Feel free to reach out or share this tutorial on [X](https://twitter.com/prefix_dev), [join our Discord](https://discord.gg/kKV8ZxyzY4), send us an [e-mail](mailto:hi@prefix.dev) or follow our [GitHub](https://github.com/prefix-dev).
