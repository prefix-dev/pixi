# Tutorial: Adding variants

In this tutorial, we will show you how to use variants in order to build a pixi package against different versions of a dependency.
Some might call this functionality, build matrix, build configurations or parameterized builds, in the conda ecosystem this is referred to as a variant.

!!! warning
    `pixi-build` is a preview feature, and will change until it is stabilized.
    Please keep that in mind when you use it for your projects.

## Why is this useful?

When we depend on a pixi package, the dependency versions of the package itself are already set.
For example, in the [C++ tutorial](cpp.md) the `python_bindings` package we built depended on Python 3.12.
Pixi would report a version conflict, if we'd add use both Python 3.11 and `python_bindings` to our workspace.
By using variants, we can add a set of allowed versions for a specific dependency.
Pixi will then resolve the package with all the different variants.

## Let's get started

In this tutorial we will continue with the result of the [workspace tutorial](workspace.md) so we can test it against multiple Python versions.
As a reminder, we ended up with a top-level `pixi.toml` containing the workspace and the Python package `rich_example`.
Our workspace then depended on `rich_example` and `python_bindings`.

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_projects/pixi_build_workspace_variants/pixi.toml:dependencies"
```

The file tree looks like this:

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

In order to allow multiple Python versions we first have to change the Python version requirement of `python_bindings` from `3.12.*` to `*`.

```toml title="packages/python_bindings/pixi.toml" hl_lines="4"
--8<-- "docs/source_files/pixi_projects/pixi_build_workspace_variants/packages/python_bindings/pixi.toml:host-dependencies"
```

1. Used to be "3.12.*"

Now, we have to specify the Python versions we want to allow.
We do that in `workspace.build-variants`:

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_projects/pixi_build_workspace_variants/pixi.toml:variants"
```

If we'd run `pixi install` now, we'd leave it up to pixi whether to use Python 3.11 or 3.12.
In practice, you'll want to create multiple environments specifying a different dependency version.
In our case this allows us to test our setup against both Python 3.11 and 3.12.


```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_projects/pixi_build_workspace_variants/pixi.toml:environments"
```

By running `pixi list` we can see the Python version used in each environment.
You can also see that the `Build` string of `python_bindings` differ between `py311` and `py312`.
That means that a different package has been built for each variant.
Since `rich_example` only contains Python source code, a single build can be used for multiple Python versions.
The package is `noarch`.
Therefore, the build string is the same.


```pwsh
$ pixi list --environment py311
Package            Version     Build               Size       Kind   Source
python             3.11.11     h9e4cc4f_1_cpython  29.2 MiB   conda  python
python_bindings    0.1.0       py311h43a39b2_0                conda  python_bindings
rich_example       0.1.0       pyhbf21a9e_0                   conda  rich_example
```

```pwsh
$ pixi list --environment py312
Package            Version     Build               Size       Kind   Source
python             3.12.8      h9e4cc4f_1_cpython  30.1 MiB   conda  python
python_bindings    0.1.0       py312h2078e5b_0                conda  python_bindings
rich_example       0.1.0       pyhbf21a9e_0                   conda  rich_example
```


## Conclusion

In this tutorial, we showed how to use variants to build multiple versions of a single package.
We built `python_bindings` for Python 3.12 and 3.13, which allows us to test whether it works properly on both Python versions.
Variants are not limited to a single dependency, you could for example try to test multiple versions of `nanobind`.

Thanks for reading! Happy Coding 🚀

Any questions? Feel free to reach out or share this tutorial on [X](https://twitter.com/prefix_dev), [join our Discord](https://discord.gg/kKV8ZxyzY4), send us an [e-mail](mailto:hi@prefix.dev) or follow our [GitHub](https://github.com/prefix-dev).
