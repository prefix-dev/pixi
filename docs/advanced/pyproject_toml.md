# `pyproject.toml` in pixi

We support the use of the `pyproject.toml` as our manifest file in pixi.
This allows the user to keep one file with all configuration.
The `pyproject.toml` file is a standard for Python projects.
We don't advise to use the `pyproject.toml` file for anything else than python projects, the `pixi.toml` is better suited for other types of projects.

## Initial setup of the `pyproject.toml` file

When you already have a `pyproject.toml` file in your project, you can run `pixi init` in a that folder. Pixi will automatically

- Add a `[tool.pixi.project]` section to the file, with the platform and channel information required by pixi;
- Add the current project as an editable pypi dependency;
- Add some defaults to the `.gitignore` and `.gitattributes` files.

If you do not have an existing `pyproject.toml` file , you can run `pixi init --format pyproject` in your project folder. In that case, pixi will create a `pyproject.toml` manifest from scratch with some sane defaults.

## Python dependency

The `pyproject.toml` file supports the `requires_python` field.
Pixi understands that field and automatically adds the version to the dependencies.

This is an example of a `pyproject.toml` file with the `requires_python` field, which will be used as the python dependency:

```toml title="pyproject.toml"
[project]
name = "my_project"
requires-python = ">=3.9"

[tool.pixi.project]
channels = ["conda-forge"]
platforms = ["linux-64", "osx-arm64", "osx-64", "win-64"]
```

Which is equivalent to:

```toml title="equivalent pixi.toml"
[project]
name = "my_project"
channels = ["conda-forge"]
platforms = ["linux-64", "osx-arm64", "osx-64", "win-64"]

[dependencies]
python = ">=3.9"
```

## Dependency section

The `pyproject.toml` file supports the `dependencies` field.
Pixi understands that field and automatically adds the dependencies to the project as `[pypi-dependencies]`.

This is an example of a `pyproject.toml` file with the `dependencies` field:

```toml title="pyproject.toml"
[project]
name = "my_project"
requires-python = ">=3.9"
dependencies = [
    "numpy",
    "pandas",
    "matplotlib",
]

[tool.pixi.project]
channels = ["conda-forge"]
platforms = ["linux-64", "osx-arm64", "osx-64", "win-64"]
```

Which is equivalent to:

```toml title="equivalent pixi.toml"
[project]
name = "my_project"
channels = ["conda-forge"]
platforms = ["linux-64", "osx-arm64", "osx-64", "win-64"]

[pypi-dependencies]
numpy = "*"
pandas = "*"
matplotlib = "*"

[dependencies]
python = ">=3.9"
```

You can overwrite these with conda dependencies by adding them to the `dependencies` field:

```toml title="pyproject.toml"
[project]
name = "my_project"
requires-python = ">=3.9"
dependencies = [
    "numpy",
    "pandas",
    "matplotlib",
]

[tool.pixi.project]
channels = ["conda-forge"]
platforms = ["linux-64", "osx-arm64", "osx-64", "win-64"]

[tool.pixi.dependencies]
numpy = "*"
pandas = "*"
matplotlib = "*"
```

This would result in the conda dependencies being installed and the pypi dependencies being ignored.
As pixi takes the conda dependencies over the pypi dependencies.

## Optional dependencies

If your python project includes groups of optional dependencies, pixi will automatically interpret them as [pixi features](../reference/pixi_manifest.md#the-feature-table) of the same name with the associated `pypi-dependencies`.

You can add them to pixi environments manually, or use `pixi init` to setup the project, which will create one environment per feature. Self-references to other groups of optional dependencies are also handled.

For instance, imagine you have a project folder with a `pyproject.toml` file similar to:

```toml
[project]
name = "my_project"
dependencies = ["package1"]

[project.optional-dependencies]
test = ["pytest"]
all = ["package2","my_project[test]"]
```

Running `pixi init` in that project folder will transform the `pyproject.toml` file into:

```toml
[project]
name = "my_project"
dependencies = ["package1"]

[project.optional-dependencies]
test = ["pytest"]
all = ["package2","my_project[test]"]

[tool.pixi.project]
channels = ["conda-forge"]
platforms = ["linux-64"] # if executed on linux

[tool.pixi.environments]
default = {features = [], solve-group = "default"}
test = {features = ["test"], solve-group = "default"}
all = {features = ["all", "test"], solve-group = "default"}
```

In this example, three environments will be created by pixi:

- **default** with 'package1' as pypi dependency
- **test** with 'package1' and 'pytest' as pypi dependencies
- **all** with 'package1', 'package2' and 'pytest' as pypi dependencies

All environments will be solved together, as indicated by the common `solve-group`, and added to the lock file. You can edit the `[tool.pixi.environments]` section manually to adapt it to your use case (e.g. if you do not need a particular environment).

## Dependency groups

If your python project includes dependency groups, pixi will automatically interpret them as [pixi features](../reference/pixi_manifest.md#the-feature-table) of the same name with the associated `pypi-dependencies`.

You can add them to pixi environments manually, or use `pixi init` to setup the project, which will create one environment per dependency group.

For instance, imagine you have a project folder with a `pyproject.toml` file similar to:

```toml
[project]
name = "my_project"
dependencies = ["package1"]

[dependency-groups]
test = ["pytest"]
docs = ["sphinx"]
dev = [{include-group = "test"}, {include-group = "docs"}]
```

Running `pixi init` in that project folder will transform the `pyproject.toml` file into:

```toml
[project]
name = "my_project"
dependencies = ["package1"]

[dependency-groups]
test = ["pytest"]
docs = ["sphinx"]
dev = [{include-group = "test"}, {include-group = "docs"}]

[tool.pixi.project]
channels = ["conda-forge"]
platforms = ["linux-64"] # if executed on linux

[tool.pixi.environments]
default = {features = [], solve-group = "default"}
test = {features = ["test"], solve-group = "default"}
docs = {features = ["docs"], solve-group = "default"}
dev = {features = ["dev"], solve-group = "default"}
```

In this example, four environments will be created by pixi:

- **default** with 'package1' as pypi dependency
- **test** with 'package1' and 'pytest' as pypi dependencies
- **docs** with 'package1', 'sphinx' as pypi dependencies
- **dev** with 'package1', 'sphinx' and 'pytest' as pypi dependencies

All environments will be solved together, as indicated by the common `solve-group`, and added to the lock file. You can edit the `[tool.pixi.environments]` section manually to adapt it to your use case (e.g. if you do not need a particular environment).

## Example

As the `pyproject.toml` file supports the full pixi spec with `[tool.pixi]` prepended an example would look like this:

```toml title="pyproject.toml"
[project]
name = "my_project"
requires-python = ">=3.9"
dependencies = [
    "numpy",
    "pandas",
    "matplotlib",
    "ruff",
]

[tool.pixi.project]
channels = ["conda-forge"]
platforms = ["linux-64", "osx-arm64", "osx-64", "win-64"]

[tool.pixi.dependencies]
compilers = "*"
cmake = "*"

[tool.pixi.tasks]
start = "python my_project/main.py"
lint = "ruff lint"

[tool.pixi.system-requirements]
cuda = "11.0"

[tool.pixi.feature.test.dependencies]
pytest = "*"

[tool.pixi.feature.test.tasks]
test = "pytest"

[tool.pixi.environments]
test = ["test"]
```

## Build-system section

The `pyproject.toml` file normally contains a `[build-system]` section. Pixi will use this section to build and install the project if it is added as a pypi path dependency.

If the `pyproject.toml` file does not contain any `[build-system]` section, pixi will fall back to [uv](https://github.com/astral-sh/uv)'s default, which is equivalent to the below:

```toml title="pyproject.toml"
[build-system]
requires = ["setuptools >= 40.8.0"]
build-backend = "setuptools.build_meta:__legacy__"
```

Including a `[build-system]` section is **highly recommended**. If you are not sure of the [build-backend](https://packaging.python.org/en/latest/tutorials/packaging-projects/#choosing-build-backend) you want to use, including the `[build-system]` section below in your `pyproject.toml` is a good starting point.
`pixi init --format pyproject` defaults to `hatchling`.
The advantages of `hatchling` over `setuptools` are outlined on its [website](https://hatch.pypa.io/latest/why/#build-backend).

```toml title="pyproject.toml"
[build-system]
build-backend = "hatchling.build"
requires = ["hatchling"]
```
