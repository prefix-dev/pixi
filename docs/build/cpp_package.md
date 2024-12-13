# Building a C++ example

This example shows how to build a C++ project with CMake and use it together with `pixi-build`.
To read more about how building packages work with pixi see the [Getting Started](./getting_started.md) guide.

## Creating a new project

To get started, create a new project with pixi:

```bash
pixi init
```

This should give you the basic `pixi.toml` to get started.

## Adding necessary sections
Add the following sections to your `pixi.toml` file:

```toml hl_lines="7 14"
--8<-- "docs/source_files/pixi_tomls/pixi_build_cpp/pixi.toml"
```

1.  Add the **preview** feature `pixi-build` that enables pixi to build the package.
