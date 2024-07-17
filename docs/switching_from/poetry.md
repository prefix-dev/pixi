# Transitioning from `poetry` to `pixi`
Welcome to the guide designed to ease your transition from `poetry` to `pixi`.
This document compares key commands and concepts between these tools, highlighting `pixi`'s unique approach to managing environments and packages.
With `pixi`, you'll experience a project-based workflow similar to `poetry` while including the `conda` ecosystem and allowing for easy sharing of your work.

## Why Pixi?
Poetry is most-likely the closed tool to pixi in terms of project management, in the python ecosystem.
`pixi` however adds the power of the conda ecosystem, allowing for a more flexible and powerful environment management.
Not limited but including the PyPI ecosystem.

## Quick look at the differences
| Task                       | Poetry                                                            | Pixi                                                                                                                                              |
|----------------------------|-------------------------------------------------------------------|---------------------------------------------------------------------------------------------------------------------------------------------------|
| Creating an Environment    | `poetry new myenv`                                                | `pixi init myenv`                                                                                                                                 |
| Running a Task             | `poetry run which python`                                         | `pixi run which python` `pixi` uses a build-in cross platform shell for run where poetry uses your shell.                                         |
| Installing a Package       | `poetry add numpy`                                                | `pixi add numpy` adds the conda variant. `pixi add --pypi numpy` adds the PyPI variant of the package.                                            |
| Uninstalling a Package     | `poetry remove numpy`                                             | `pixi remove numpy`                                                                                                                               |
| Building a package         | `poetry build`                                                    | We've yet to implement package building and publishing                                                                                            |
| Publishing a package       | `poetry publish`                                                  | We've yet to implement package building and publishing                                                                                            |
| Reading the pyproject.toml | `[tool.poetry]`                                                   | `[tool.pixi]`                                                                                                                                     |
| Defining dependencies      | `[tool.poetry.dependencies]`                                      | `[tool.pixi.dependencies]` for conda, `[tool.pixi.pypi-dependencies]` or `[project.dependencies]` for PyPI dependencies                           |
| Dependency definition      | - `numpy = "^1.2.3"`<br/>- `numpy = "~1.2.3"`<br/>- `numpy = "*"` | - `numpy = ">=1.2.3 <2.0.0"`<br/>- `numpy = ">=1.2.3 <1.3.0"`<br/>- `numpy = "*"`                                                                 |
| Lock file                  | `poetry.lock`                                                     | `pixi.lock`                                                                                                                                       |
| Environment directory       | `~/.cache/pypoetry/virtualenvs/myenv`                             | `./.pixi` Defaults to the project folder, move this using the [`detached-environments`](../reference/pixi_configuration.md#detached-environments) |

## Support both `poetry` and `pixi` in my project
You can use both `poetry` and `pixi` in the same project, they will not touch each other's parts of the configuration or system. 
It's best to duplicate the dependencies, basically making an exact copy of the `tool.poetry.dependencies` into `tool.pixi.pypi-dependencies`.
Make sure that `python` is only defined in the `tool.pixi.dependencies` and not in the `tool.pixi.pypi-dependencies`.

!!! Danger
    It's possible to use `poetry` in `pixi` environments but this is advised against. 