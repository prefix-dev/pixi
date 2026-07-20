In this tutorial, we will show you how to use variants in order to build a Pixi package against different versions of a dependency. Some might call this functionality, build matrix, build configurations or parameterized builds, in the conda ecosystem this is referred to as a variant.

Warning

`pixi-build` is a preview feature, and will change until it is stabilized. Please keep that in mind when you use it for your projects.

## Why is This Useful?

When we depend on a Pixi package, the dependency versions of the package itself are already set. For example, in the [C++ tutorial](../cpp/) the `cpp_math` package we built depended on Python 3.12. Pixi would report a version conflict, if we'd add use both Python 3.11 and `cpp_math` to our workspace. By using variants, we can add a set of allowed versions for a specific dependency. Pixi will then resolve the package with all the different variants.

## Let's Get Started

In this tutorial we will continue with the result of the [workspace tutorial](../workspace/) so we can test it against multiple Python versions. As a reminder, we ended up with a top-level `pixi.toml` containing the workspace and the Python package `python_rich`. Our workspace then depended on `python_rich` and `cpp_math`.

pixi.toml

```toml
[dependencies]
python_rich = { path = "." }
```

The file tree looks like this:

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

In order to allow multiple Python versions we first have to change the Python version requirement of `cpp_math` from `3.12.*` to `*`.

packages/cpp_math/pixi.toml

```toml
[package.host-dependencies]
cmake = ">=3.20, <3.27"
nanobind = ">=2.4.0, <2.5.0"
python = "*"                 # (1)!
```

1. Used to be "3.12.\*"

Now, we have to specify the Python versions we want to allow. We do that in `workspace.build-variants`:

pixi.toml

```toml
[workspace.build-variants]
python = ["3.11.*", "3.12.*"]
```

If we'd run `pixi install` now, we'd leave it up to Pixi whether to use Python 3.11 or 3.12. In practice, you'll want to create multiple environments specifying a different dependency version. In our case this allows us to test our setup against both Python 3.11 and 3.12.

pixi.toml

```toml
[feature.py311.dependencies]
python = "3.11.*"

[feature.py312.dependencies]
python = "3.12.*"

[environments]
py311 = ["py311"]
py312 = ["py312"]
```

By running `pixi list` we can see the Python version used in each environment. You can also see that the `Build` string of `cpp_math` differ between `py311` and `py312`. That means that a different package has been built for each variant. Since `python_rich` only contains Python source code, a single build can be used for multiple Python versions. The package is `noarch`. Therefore, the build string is the same.

```pwsh
$ pixi list --environment py311
Package            Version     Build               Size       Kind   Source
python             3.11.11     h9e4cc4f_1_cpython  29.2 MiB   conda  python
cpp_math           0.1.0       py311h43a39b2_0                conda  cpp_math
python_rich        0.1.0       pyhbf21a9e_0                   conda  python_rich
```

```pwsh
$ pixi list --environment py312
Package            Version     Build               Size       Kind   Source
python             3.12.8      h9e4cc4f_1_cpython  30.1 MiB   conda  python
cpp_math           0.1.0       py312h2078e5b_0                conda  cpp_math
python_rich        0.1.0       pyhbf21a9e_0                   conda  python_rich
```

## Variants Pixi Sets for You

Not every variant has to be spelled out in `[workspace.build-variants]`. When you build a package, Pixi fills in a few variants automatically based on the platform you build for and its [system requirements](../../workspace/system_requirements/). An explicit `[workspace.build-variants]` entry always wins over a value Pixi derives, so you can override any of these by hand.

### `target_platform`

Every build is tagged with the platform it is built for, so a build made for `linux-64` and one made for `osx-arm64` stay distinct. You never set this yourself; it comes from the platform being built.

### `c_stdlib` and `c_stdlib_version`

Build backends pin the minimum OS/libc target through the recipe's `stdlib("c")` function, which resolves against the `c_stdlib` and `c_stdlib_version` variants. Rather than making you repeat this in `[workspace.build-variants]`, Pixi derives the pair from the target platform's system requirements (recorded as the `__osx` and `__glibc` virtual packages):

| Subdir    | Virtual package | `c_stdlib`                 | `c_stdlib_version`    |
| --------- | --------------- | -------------------------- | --------------------- |
| `osx-*`   | `__osx`         | `macosx_deployment_target` | the `__osx` version   |
| `linux-*` | `__glibc`       | `sysroot`                  | the `__glibc` version |

Platforms declared as a bare subdir string carry Pixi's portable defaults (`__glibc = "2.28"`, `__osx = "13.0"`), so the derivation works even when you don't declare system requirements explicitly.

The providers `macosx_deployment_target` and `sysroot` are conda-forge packages, so this derivation only applies when one of your channels is conda-forge. It is skipped on Windows (which has no meaningful stdlib version), and musl (`__musl`) and CUDA (`__cuda`) are not derived yet.

## Conclusion

In this tutorial, we showed how to use variants to build multiple versions of a single package. We built `cpp_math` for Python 3.12 and 3.13, which allows us to test whether it works properly on both Python versions. Variants are not limited to a single dependency, you could for example try to test multiple versions of `nanobind`.

On top of adding variants inline, they can also be included as files. Check out the [reference](../../reference/pixi_manifest/#build-variants-files-optional) to learn more!

Thanks for reading! Happy Coding 🚀

Any questions? Feel free to reach out or share this tutorial on [X](https://twitter.com/prefix_dev), [join our Discord](https://discord.gg/kKV8ZxyzY4), send us an [e-mail](mailto:hi@prefix.dev) or follow our [GitHub](https://github.com/prefix-dev).
