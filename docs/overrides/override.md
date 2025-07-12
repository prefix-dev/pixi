## Overview

Sometimes our direct dependency declares outdated intermediate dependency or is too tight to solve with other direct dependencies. In this case, we can override the intermediate dependency in our `pyproject.toml`or `pixi.toml` file.

> [!Note] This option is not recommended unless you know what you are doing, as uv will ignore all the version constraints of the dependency and use the version you specified.

## Example
### Override a dependency version
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

### Override a dependency version in a specific feature
it can also be specified in feature level,
```toml
[features.dev.pypi-options.dependency-overrides]
numpy = ">=2.0.0"
```
This will override the version of `numpy` used by all dependencies in the `dev` feature to be at least `2.0.0`, regardless of what the dependencies specify when the `dev` feature is enabled.

### Interact with other overrides
For a specific environment, all the `dependency-overrides` defined in different features will be combined in the order they were when defining the environment.

If the same dependency is overridden multiple times, we'll use the override from the **prior** feature in that environment.

Also, the default feature will always come, and come last in the list of all overrides.

```toml
# pixi.toml
[pypi-options]
dependency-overrides = { numpy = ">=2.1.0" }

[pypi-dependencies]
numpy = ">=1.25.0"

[feature.dev.pypi-options.dependency-overrides]
numpy = "==2.0.0"

[feature.outdated.pypi-options.dependency-overrides]
numpy = "==1.21.0"

[environments]
dev = ["dev"]
outdated = ["outdated"]
conflict_a=["outdated", "dev"]
conflict_b=["dev","outdated"]
```
the following constrains are merged out:
default: `numpy >= 2.1.0`
dev: `numpy == 2.0.0`
outdated: `numpy == 1.21.0`
conflict_a: `numpy == 1.21.0` (from `outdated`)
conflict_b: `numpy == 2.0.0` (from `dev`)

This may contrast with the intuition that all overrides are applied and combined to a result, but it is done this way to avoid conflicts and confusion. Since users are granted fully control over the overrides, it is up to ourselves to choose the right overrides for the environment.
