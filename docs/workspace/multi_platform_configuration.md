[Pixi's vision](../misc/vision.md) includes being supported on all major platforms. Sometimes that needs some extra configuration to work well.
On this page, you will learn what you can configure to align better with the platform you are making your application for.

Here is an example manifest file that highlights some of the features:

=== "`pixi.toml`"
    ```toml title="pixi.toml"
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
=== "`pyproject.toml`"
    ```toml title="pyproject.toml"
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

The `workspace.platforms` defines which platforms your workspace supports.
When multiple platforms are defined, Pixi determines which dependencies to install for each platform individually.
All of this is stored in a lock file.

Running `pixi install` on a platform that is not configured will warn the user that it is not setup for that platform:

```shell
❯ pixi install
 WARN Not installing dependency for (default) on current platform: (osx-arm64) as it is not part of this project's supported platforms.
```

## Declaring virtual packages per platform

A bare-string entry like `"linux-64"` is shorthand for "the conda subdir
`linux-64` with whatever virtual packages Pixi auto-detects on the host".
You can also describe a platform as an inline table to pin the
[virtual packages](https://docs.conda.io/projects/conda/en/latest/user-guide/tasks/manage-virtual.html)
the solver should treat as available -- for example a CUDA toolkit version or
a glibc minimum.

```toml title="pixi.toml"
[workspace]
platforms = [
  "osx-arm64",
  { platform = "linux-64", cuda = "12.0", libc = "2.28" },
  { name = "jetson-nano", platform = "linux-aarch64", cuda = "12.8" },
]
```

Each inline-table entry has:

- `platform` -- the conda subdir the entry targets (e.g. `linux-64`,
  `osx-arm64`). Required.
- `name` -- optional workspace-scoped identifier the platform is referenced
  by elsewhere (in `feature.<name>.platforms`, in lockfile rows, in CLI
  commands). When omitted, Pixi synthesises a name from `platform` plus the
  declared virtual packages, so two entries that declare the same set in
  different key order share the same identifier.
- Friendly keys for the common virtual packages: `cuda`, `archspec`, `libc`,
  `linux`, `macos`, `windows`. Each maps onto the matching `__name` conda
  virtual package (`cuda` -> `__cuda`, `libc` -> `__glibc`, `macos` ->
  `__osx`, etc.).
- For virtual packages without a friendly key, a raw `__name = "version"`
  entry is also accepted as an escape hatch.

A feature's `platforms` array is a list of names that must each resolve to a
workspace platform (or be a bare conda subdir, which Pixi treats as an alias
for that subdir). This is how you bind a feature to the rich variant:

```toml title="pixi.toml"
[workspace]
platforms = [
  "osx-arm64",
  { platform = "linux-64", cuda = "12.0" },
]

[feature.gpu]
platforms = ["linux-64-cuda-12-0"]  # the synthesised name for the entry above
```

### Managing platforms from the CLI

[`pixi workspace platform`](../reference/cli/pixi/workspace/platform.md) is
the CLI surface for these entries:

- `pixi workspace platform add <PLATFORM> [--cuda 12.0] [--libc 2.28] ...`
  appends bare subdirs or rich platforms.
- `pixi workspace platform edit <NAME> [--cuda 12.1] [--remove-virtual-package __libc]`
  mutates a custom platform's declared virtual packages.
- `pixi workspace platform list` / `show` inspect what is declared.
- `pixi workspace platform remove <NAME>` drops an entry.

The mutating subcommands keep `pixi.lock` in sync.

## Target specifier

With the target specifier, you can overwrite the original configuration specifically for a single platform.
If you are targeting a specific platform in your target specifier that was not specified in your `workspace.platforms` then Pixi will throw an error.

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

Pixi's vision is to enable completely cross-platform workspaces, but you often need to run tools that are not built by your projects.
Generated activation scripts are often in this category, default scripts in unix are `bash` and for windows they are `bat`

To deal with this, you can define your activation scripts using the target definition.

```toml title="pixi.toml"
[activation]
scripts = ["setup.sh", "local_setup.bash"]

[target.win-64.activation]
scripts = ["setup.bat", "local_setup.bat"]
```
When this workspace is used on `win-64` it will only execute the target scripts not the scripts specified in the default `activation.scripts`
