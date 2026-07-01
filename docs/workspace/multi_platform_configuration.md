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

!!! info "Replaces `[system-requirements]`"
    These inline-table entries are the recommended way to declare CUDA, glibc,
    macOS, archspec, and similar constraints. The older `[system-requirements]` table
    still parses but is deprecated; see
    [Migrating from `[system-requirements]`](./system_requirements.md) for the
    equivalent forms.

```toml title="pixi.toml"
[workspace]
platforms = [
  "osx-arm64",
  { platform = "linux-64", cuda = "12.0", glibc = "2.28" },
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
- Friendly keys for the common virtual packages: `cuda`, `archspec`, `glibc`,
  `linux`, `macos` (alias `osx`), `windows`. Each maps onto the matching
  `__name` conda virtual package (`cuda` -> `__cuda`, `glibc` -> `__glibc`,
  `macos` -> `__osx`, etc.).
- `cuda` also accepts a `{ driver, arch }` table that declares the CUDA driver
  version (`__cuda`) together with the GPU compute capability (`__cuda_arch`):

    ```toml title="pixi.toml"
    platforms = [
      { name = "gpu", platform = "linux-64", cuda = { driver = "12.0", arch = "8.6" } },
    ]
    ```

    `driver` is exactly equivalent to the bare `cuda = "12.0"` form. Per the
    conda CEP, `__cuda_arch` is meaningless without `__cuda`, so `arch` requires
    `driver` -- declaring `arch` (or a raw `__cuda_arch`) alone is rejected.
- For virtual packages without a friendly key, a raw `__name = "version"`
  entry is also accepted as an escape hatch. Only the virtual packages pixi
  knows how to override (`__win`, `__osx`, `__linux`, `__cuda`, `__archspec`,
  and the libc family `__glibc`/`__musl`/`__eglibc`) take effect at detection;
  any other raw `__name` is stored but ignored when checking host
  compatibility.

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

!!! note "Platform names in `pixi.lock`"
    Rich platforms are written to `pixi.lock` under short aliases (`p1`, `p2`,
    ...) instead of their full names, to keep the lock file compact. Pixi maps
    these back to the manifest entries by their contents (subdir plus declared
    virtual packages) when the lock file is read, so the aliases never need to
    be understood by hand. The real names stay in `pixi.toml`.

### Managing platforms from the CLI

[`pixi workspace platform`](../reference/cli/pixi/workspace/platform.md) is
the CLI surface for these entries:

- `pixi workspace platform add <PLATFORM> [--cuda 12.0] [--cuda-arch 8.6] [--glibc 2.28] ...`
  appends bare subdirs or rich platforms. `--cuda-arch` requires `--cuda` (or
  an existing `__cuda`) and serializes as `cuda = { driver, arch }`.
- `pixi workspace platform edit <NAME> [--cuda 12.1] [--remove-virtual-package __glibc]`
  mutates a custom platform's declared virtual packages.
- `pixi workspace platform list` inspects what is declared.
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

### Wildcard platform selectors

When several [workspace platforms](#declaring-virtual-packages-per-platform) share configuration, you can match them with a `*` wildcard in the target selector instead of repeating each block.
The pattern is matched against the platform *name*, so it is most useful together with custom platform names:

```toml title="pixi.toml"
[workspace]
platforms = [
  { name = "cuda-win-64", platform = "win-64", cuda = "12" },
  { name = "cuda-linux-64", platform = "linux-64", cuda = "12" },
  "win-64",
  "linux-64",
]

[target."cuda-*".tasks]
test = "python test.py --cuda"
train = "python train.py --cuda"
```

Here both `cuda-win-64` and `cuda-linux-64` pick up the `test` and `train` tasks, while the bare `win-64` and `linux-64` platforms do not.

A few details:

- `*` is the only metacharacter and matches any run of characters. Patterns are matched in full and are case-sensitive (`cuda-*`, `*-64`, `*cuda*`).
- When more than one selector matches a platform, the one defined **later** in the manifest wins, the same way `[target.linux]` and `[target.linux-64]` already combine. Place a specific `[target.cuda-win-64]` override *after* the `[target."cuda-*"]` block.
- Wildcards are only allowed on workspace and feature targets. They are rejected in `[package.target]` and `[package.build.target]`, which resolve by subdir.
