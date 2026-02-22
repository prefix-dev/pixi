# Recursive run dependencies

This example shows that source packages can depend on other source packages.

The pixi workspace at the root includes the package `root` as a dependency and nothing else:

```toml
[dependencies]
root = { path = "src/root" }
```

The root package has a run dependency on another package called `depend`:

```toml
[tool.pixi.package.run-dependencies]
depend = { path = "../depend" }
```

When installing the default environment both packages are installed because of this.
