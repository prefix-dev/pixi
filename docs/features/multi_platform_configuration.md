---
part: pixi/advanced
title: Multi platform config
description: Learn how to set up for different platforms/OS's
---

[Pixi's vision](../vision.md) includes being supported on all major platforms. Sometimes that needs some extra configuration to work well.
On this page, you will learn what you can configure to align better with the platform you are making your application for.

Here is an example manifest file that highlights some of the features:

=== "`pixi.toml`"
    ```toml title="pixi.toml"
    [project]
    # Default project info....
    # A list of platforms you are supporting with your package.
    platforms = ["win-64", "linux-64", "osx-64", "osx-arm64"]

    [dependencies]
    python = ">=3.8"

    [target.win-64.dependencies]
    # Overwrite the needed python version only on win-64
    python = "3.7"


    [activation]
    scripts = ["setup.sh"]

    [target.win-64.activation]
    # Overwrite activation scripts only for windows
    scripts = ["setup.bat"]
    ```
=== "`pyproject.toml`"
    ```toml title="pyproject.toml"
    [tool.pixi.project]
    # Default project info....
    # A list of platforms you are supporting with your package.
    platforms = ["win-64", "linux-64", "osx-64", "osx-arm64"]

    [tool.pixi.dependencies]
    python = ">=3.8"

    [tool.pixi.target.win-64.dependencies]
    # Overwrite the needed python version only on win-64
    python = "~=3.7.0"


    [tool.pixi.activation]
    scripts = ["setup.sh"]

    [tool.pixi.target.win-64.activation]
    # Overwrite activation scripts only for windows
    scripts = ["setup.bat"]
    ```

## Platform definition

The `project.platforms` defines which platforms your project supports.
When multiple platforms are defined, pixi determines which dependencies to install for each platform individually.
All of this is stored in a lock file.

Running `pixi install` on a platform that is not configured will warn the user that it is not setup for that platform:

```shell
❯ pixi install
  × the project is not configured for your current platform
   ╭─[pixi.toml:6:1]
 6 │ channels = ["conda-forge"]
 7 │ platforms = ["osx-64", "osx-arm64", "win-64"]
   ·             ────────────────┬────────────────
   ·                             ╰── add 'linux-64' here
 8 │
   ╰────
  help: The project needs to be configured to support your platform (linux-64).
```

## Target specifier

With the target specifier, you can overwrite the original configuration specifically for a single platform.
If you are targeting a specific platform in your target specifier that was not specified in your `project.platforms` then pixi will throw an error.

### Dependencies

It might happen that you want to install a certain dependency only on a specific platform, or you might want to use a different version on different platforms.

```toml title="pixi.toml"
[dependencies]
python = ">=3.8"

[target.win-64.dependencies]
msmpi = "*"
python = "3.8"
```

In the above example, we specify that we depend on `msmpi` only on Windows.
We also specifically want `python` on `3.8` when installing on Windows.
This will overwrite the dependencies from the generic set of dependencies.
This will not touch any of the other platforms.

You can use pixi's cli to add these dependencies to the manifest file.

```shell
pixi add --platform win-64 posix
```

This also works for the `host` and `build` dependencies.

```bash
pixi add --host --platform win-64 posix
pixi add --build --platform osx-64 clang
```

Which results in this.

```toml title="pixi.toml"
[target.win-64.host-dependencies]
posix = "1.0.0.*"

[target.osx-64.build-dependencies]
clang = "16.0.6.*"
```

### Activation

Pixi's vision is to enable completely cross-platform projects, but you often need to run tools that are not built by your projects.
Generated activation scripts are often in this category, default scripts in unix are `bash` and for windows they are `bat`

To deal with this, you can define your activation scripts using the target definition.

```toml title="pixi.toml"
[activation]
scripts = ["setup.sh", "local_setup.bash"]

[target.win-64.activation]
scripts = ["setup.bat", "local_setup.bat"]
```
When this project is run on `win-64` it will only execute the target scripts not the scripts specified in the default `activation.scripts`
