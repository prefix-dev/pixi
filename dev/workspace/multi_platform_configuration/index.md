[Pixi's vision](../../misc/vision/) includes being supported on all major platforms. Sometimes that needs some extra configuration to work well. On this page, you will learn what you can configure to align better with the platform you are making your application for.

Here is an example manifest file that highlights some of the features:

pixi.toml

```toml
[workspace]
# Default workspace info....
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

pyproject.toml

```toml
[tool.pixi.workspace]
# Default workspace info....
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

The `workspace.platforms` defines which platforms your workspace supports. When multiple platforms are defined, Pixi determines which dependencies to install for each platform individually. All of this is stored in a lock file.

Running `pixi install` on a platform that is not configured will warn the user that it is not setup for that platform:

```shell
❯ pixi install
 WARN Not installing dependency for (default) on current platform: (osx-arm64) as it is not part of this project's supported platforms.

```

## Target specifier

With the target specifier, you can overwrite the original configuration specifically for a single platform. If you are targeting a specific platform in your target specifier that was not specified in your `workspace.platforms` then Pixi will throw an error.

### Dependencies

It might happen that you want to install a certain dependency only on a specific platform, or you might want to use a different version on different platforms.

pixi.toml

```toml
[dependencies]
python = ">=3.8"
[target.win-64.dependencies]
msmpi = "*"
python = "3.8"

```

In the above example, we specify that we depend on `msmpi` only on Windows. We also specifically want `python` on `3.8` when installing on Windows. This will overwrite the dependencies from the generic set of dependencies. This will not touch any of the other platforms.

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

pixi.toml

```toml
[target.win-64.host-dependencies]
posix = "1.0.0.*"
[target.osx-64.build-dependencies]
clang = "16.0.6.*"

```

### Activation

Pixi's vision is to enable completely cross-platform workspaces, but you often need to run tools that are not built by your projects. Generated activation scripts are often in this category, default scripts in unix are `bash` and for windows they are `bat`

To deal with this, you can define your activation scripts using the target definition.

pixi.toml

```toml
[activation]
scripts = ["setup.sh", "local_setup.bash"]
[target.win-64.activation]
scripts = ["setup.bat", "local_setup.bat"]

```

When this workspace is used on `win-64` it will only execute the target scripts not the scripts specified in the default `activation.scripts`
