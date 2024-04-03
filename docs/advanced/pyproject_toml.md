# `pyproject.toml` in pixi
We support the use of the `pyproject.toml` as our manifest file in pixi.
This allows the user to keep one file with all configuration.
The `pyproject.toml` file is a standard for Python projects.
We don't advise to use the `pyproject.toml` file for anything else than python projects, the `pixi.toml` is better suited for other types of projects.

## Initial setup of the `pyproject.toml` file
When you already have a `pyproject.toml` file in your project, you can add the following section to it:
```toml
[tool.pixi.project]
name = "my_project"
channels = ["conda-forge"]
platforms = ["linux-64", "osx-arm64", "osx-64", "win-64"]
```
This is the minimum requirement for pixi to understand and parse the project.

However, it is recommended you use `pixi init` in a folder that has a `pyproject.toml` file. Pixi will automatically

 - Add the above `[tool.pixi.project]` section to the file, auto-detecting your current platform;
 - Add the current project as an editable pypi dependency;
 - Add some defaults to the `.gitignore` and `.gitattributes` file.

## Python dependency
The `pyproject.toml` file supports the `requires_python` field.
Pixi understands that field and automatically adds the version to the dependencies.

This is an example of a `pyproject.toml` file with the `requires_python` field, which will be used as the python dependency:
```toml title="pyproject.toml"
[project]
name = "my_project"
requires-python = ">=3.9"

[tool.pixi.project]
name = "my_project"
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
name = "my_project"
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
name = "my_project"
channels = ["conda-forge"]
platforms = ["linux-64", "osx-arm64", "osx-64", "win-64"]

[tool.pixi.dependencies]
numpy = "*"
pandas = "*"
matplotlib = "*"
```
This would result in the conda dependencies being installed and the pypi dependencies being ignored.
As pixi takes the conda dependencies over the pypi dependencies.

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
name = "my_project"
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
The `pyproject.toml` file normally contains a `[build-system]` section.  Pixi will use this section to build and install the project if it is added as a pypi path dependency.

If the `pyproject.toml` file does not contain any `[build-system]` section, pixi will fall back to [uv](https://github.com/astral-sh/uv)'s default, which is equivalent to the below:

```toml title="pyproject.toml"
[build-system]
requires = ["setuptools >= 40.8.0"]
build-backend = "setuptools.build_meta:__legacy__"
```
Including a `[build-system]` section is **highly recommended**. If you are not sure of the [build-backend](https://packaging.python.org/en/latest/tutorials/packaging-projects/#choosing-build-backend) you want to use, including the `[build-system]` section below in your `pyproject.toml` is a good starting point

```toml title="pyproject.toml"
[build-system]
requires = ["setuptools"]
build-backend = "setuptools.build_meta"
```
