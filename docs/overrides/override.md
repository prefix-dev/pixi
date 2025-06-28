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
If there's another override for the same dependency, the override in the most specific **feature** will be used.
For example, if you have the following overrides:
```toml
[tool.pixi.pypi-options.dependency-overrides]
numpy = ">=2.1.0"
[features.dev.pypi-options.dependency-overrides]
numpy = ">=2.0.0"
```
When the `dev` feature is enabled, the version of `numpy` used will be `>=2.0.0`, otherwise it will be `>=2.1.0`.
This may contrast with the intuition that all overrides are applied, but it is done this way to avoid conflicts and confusion. Since users are granted fully control over the overrides, it is up to them to ensure that the overrides do not conflict with each other.
