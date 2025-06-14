## Overview

Sometimes our dependency (direct dependency) declares outdated dependency (intermediate dependency), or is too tight to solve with other direct dependencies. In this case, we can override the intermediate dependency in our `pyproject.toml`or `pixi.toml` file.

Note that this is not recommended unless you know what you are doing, as uv will ignore all the version constraints of the dependency and use the version you specified.

## Example

```toml
# pyproject.toml
[tool.pixi.pypi-options.dependency-overrides]
numpy = ">=2.0.0"
```
or in `pixi.toml`:

```toml
# pixi.toml
[pypi-options.dependency-overrides]
numpy = ">=2.0.0"
```
This will override the version of `numpy` used by all dependencies to be at least `2.0.0`, regardless of what the dependencies specify.
This is useful if you need a specific version of a library that is not compatible with the versions specified by your dependencies.

it can also be specified in feature level,
```toml
[features.dev.pypi-options.dependency-overrides]
numpy = ">=2.0.0"
```
This will override the version of `numpy` used by all dependencies in the `dev` feature to be at least `2.0.0`, regardless of what the dependencies specify when the `dev` feature is enabled.
