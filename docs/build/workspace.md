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

## Sharing Versions Across Members

Once a workspace grows past a couple of members, the same build backend,
language runtime, and sibling-package paths tend to repeat in every
`pixi.toml`.
A `[workspace.dependencies]` pool lets you declare those specs once and have
each member opt in per entry with `{ workspace = true }`.
See [Workspace Dependencies](workspace_dependencies.md) for the syntax,
override rules, and error semantics.

## Publishing the Workspace

To publish a workspace's packages, opt each of them in with `publish = true`
in its `[package]` section:

```toml title="packages/cpp_math/pixi.toml"
[package]
name = "cpp_math"
publish = true
```

`pixi publish` walks the workspace directory tree, finds every package that
opts in, and builds and uploads them in dependency order.
The discovery respects ignore files such as `.gitignore` and skips
subdirectories that contain their own workspace.
The set must be self-contained: every source dependency of a published
package has to opt in as well, and `pixi publish` fails otherwise.
This guarantees that the target channel never ends up with a package whose
dependencies were not uploaded.

See [`pixi publish`](../reference/cli/pixi/publish.md) for the full
behavior, including single-package publishes with `--path`.

## Conclusion

In this tutorial, we created a Pixi workspace containing two packages.
The manifest of `python_rich` describes the workspace as well as the package, with `cpp_math` only the `package` section is used.
Feel free to add more packages, written in different languages to this workspace!

Thanks for reading! Happy Coding 🚀

Any questions? Feel free to reach out or share this tutorial on [X](https://twitter.com/prefix_dev), [join our Discord](https://discord.gg/kKV8ZxyzY4), send us an [e-mail](mailto:hi@prefix.dev) or follow our [GitHub](https://github.com/prefix-dev).
