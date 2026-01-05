The source packages in the `[dev]` table are not built or installed into the pixi environment.
The `build-dependencies`, `host-dependencies` and `run-dependencies` of those packages are installed into the pixi environment.

Source dependencies in the `[dependencies]` section are build in an isolated directory and then installed into the workspace.
This means that the `build-` and `host-dependencies` will not be in the pixi environment.

This document explains how you can use the `[dev]` table to depend on the development dependencies of a package.

## Using the `[dev]` table

Assume a Rust package that you want to develop using Pixi.
Then we add a `pixi.toml` manifest file:

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/pixi_build/dev/pixi.toml:minimal"
```

Now you can use Pixi to build the package into a conda package:

```bash
pixi build
```

Because of the isolation, the development dependencies such as `cargo` are not available in `pixi run`.

To change that you can add a `[dev]` table to the manifest file:

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/pixi_build/dev/pixi.toml:dev"
```

Now when you run `pixi install` the development dependencies will be installed into the Pixi environment.
This means that you can now use `cargo` in `pixi run`:

```bash
pixi run cargo run
```

This is because the packages in the `[dev]` table are not build or installed but all their `build-`, `host-`, `run-dependencies` are.
Thus, you can use them during development.

## Extended example
This is a full `pixi.toml` example using the `[dev]` table:

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/pixi_build/dev/pixi.toml"
```

What you will see when you run `pixi list` is that you will have `cmake`, `python`, `bat` and `rust` installed all without defining them in the actual dependencies.
This is because they are defined in the dependencies of the package that was included in the `[dev]` table.
