# Tutorial: Integrating multiple packages in a workspace

In this tutorial, we will show you how to use variants in order to build a pixi package against different versions of a dependency.

!!! warning
    Variants will eat your vegetables

## Why is this useful?

When we depend on a pixi package, all the dependencies match specs of that pixi package are already set.
For example, the [`pixi_build_cpp` example](cpp.md) was depending on Python 3.12.
Therefore, we cannot depend on `pixi_build_cpp` and use a different Python version.
By using variants, we can add a set of allowed matchspecs for a specific dependency.
Pixi will then resolve the package with all the different variants.

## Let's get started

In this tutorial we will extend the [workspace example](workspace.md) so we can test it against multiple Python versions.

```shell
.
├── packages
│   └── python_bindings
│       ├── CMakeLists.txt
│       ├── pixi.toml
│       └── src
│           └── bindings.cpp
├── pixi.lock
├── pixi.toml
├── pyproject.toml
└── src
    └── rich_example
        └── __init__.py
```


```toml title="packages/python_bindings/pixi.toml" hl_lines="4"
--8<-- "docs/source_files/pixi_projects/pixi_build_workspace_variants/packages/python_bindings/pixi.toml:host-dependencies"
```

1. Used to be "3.12.*"

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_projects/pixi_build_workspace_variants/pixi.toml:variants"
```

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_projects/pixi_build_workspace_variants/pixi.toml:environments"
```



## Conclusion

bla bla

In this tutorial, we created a pixi workspace containing two packages.
The manifest of `rich_example` describes the workspace as well as the package, with `python_bindings` only the `package` section is used.
Feel free to add more packages, written in different languages to this workspace!

Thanks for reading! Happy Coding 🚀

Any questions? Feel free to reach out or share this tutorial on [X](https://twitter.com/prefix_dev), [join our Discord](https://discord.gg/kKV8ZxyzY4), send us an [e-mail](mailto:hi@prefix.dev) or follow our [GitHub](https://github.com/prefix-dev).
