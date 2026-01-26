In this tutorial, we will show you how to integrate multiple Pixi packages into a single workspace.

!!! warning
    `pixi-build` is a preview feature, and will change until it is stabilized.
    Please keep that in mind when you use it for your projects.

## Why is This Useful?

The packages coming from conda channels are already built and ready to use.
If you want to depend on a package you therefore typically get that package from such a channel.
However, there are situations where you want to depend on the source of a package.
This is the case for example if you want to develop on multiple packages within the same repository.
Or if you need the changes of an unreleased version of one of your dependencies.

## Let's Get Started

In this tutorial we will showcase how to develop two packages in one workspace.
For that we will use the `python_rich` Python package developed in chapter [Building a Python package](python.md) and let it depend on the `cpp_math` C++ package developed in chapter [Building a C++ package](cpp.md).

We will start with the original setup of `python_rich` and copy `cpp_math` into a folder called `packages`.
The source directory structure now looks like this:

```shell
.
├── packages
│   └── cpp_math
│       ├── CMakeLists.txt
│       ├── pixi.toml
│       └── src
│           └── math.cpp
├── pixi.lock
├── pixi.toml
├── pyproject.toml
└── src
    └── python_rich
        └── __init__.py
```

Within a Pixi manifest, you can manage a workspace and/or describe a package.
In the case of `python_rich` we choose to do both, so the only thing we have to add `cpp_math` as a [run dependency](../reference/pixi_manifest.md#run-dependencies) of `python_rich`.

```py title="pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/pixi_build/workspace/pixi.toml:run-dependencies"
```

We only want to use the `workspace` table of the top-level manifest.
Therefore, we can remove the workspace section in the manifest of `cpp_math`.

```diff title="packages/cpp_math/pixi.toml"
-[workspace]
-channels = ["https://prefix.dev/conda-forge"]
-platforms = ["osx-arm64", "osx-64", "linux-64", "win-64"]
-preview = ["pixi-build"]
-
-[dependencies]
-cpp_math = { path = "." }
-
-[tasks]
-start = "python -c 'import cpp_math as b; print(b.add(1, 2))'"
```


There is actually one problem with `python_rich`.
The age of every person is off by one year!

```
┏━━━━━━━━━━━━━━┳━━━━━┳━━━━━━━━━━━━━┓
┃ name         ┃ age ┃ city        ┃
┡━━━━━━━━━━━━━━╇━━━━━╇━━━━━━━━━━━━━┩
│ John Doe     │ 30  │ New York    │
│ Jane Smith   │ 25  │ Los Angeles │
│ Tim de Jager │ 35  │ Utrecht     │
└──────────────┴─────┴─────────────┘
```

We need to add one year to the age of every person.
Luckily `cpp_math` exposes a function `add` which allows us to do exactly that.


```py title="src/python_rich/__init__.py"
--8<-- "docs/source_files/pixi_workspaces/pixi_build/workspace/src/python_rich/__init__.py"
```

If you run `pixi run start`, the age of each person should now be accurate:

```
┏━━━━━━━━━━━━━━┳━━━━━┳━━━━━━━━━━━━━┓
┃ name         ┃ age ┃ city        ┃
┡━━━━━━━━━━━━━━╇━━━━━╇━━━━━━━━━━━━━┩
│ John Doe     │ 31  │ New York    │
│ Jane Smith   │ 26  │ Los Angeles │
│ Tim de Jager │ 36  │ Utrecht     │
└──────────────┴─────┴─────────────┘
```

## Conclusion

In this tutorial, we created a Pixi workspace containing two packages.
The manifest of `python_rich` describes the workspace as well as the package, with `cpp_math` only the `package` section is used.
Feel free to add more packages, written in different languages to this workspace!

Thanks for reading! Happy Coding 🚀

Any questions? Feel free to reach out or share this tutorial on [X](https://twitter.com/prefix_dev), [join our Discord](https://discord.gg/kKV8ZxyzY4), send us an [e-mail](mailto:hi@prefix.dev) or follow our [GitHub](https://github.com/prefix-dev).
