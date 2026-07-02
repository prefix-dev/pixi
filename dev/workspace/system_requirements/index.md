`[system-requirements]` is deprecated

Declare these constraints directly on `[workspace].platforms` instead -- see [Declaring virtual packages per platform](../multi_platform_configuration/#declaring-virtual-packages-per-platform) for the inline-table syntax and the matching [`pixi workspace platform`](../../reference/cli/pixi/workspace/platform/) CLI. Existing `[system-requirements]` tables are still parsed and migrated transparently, so older manifests keep working, but new manifests should use the per-platform form.

# Migrating from `[system-requirements]`

The `[system-requirements]` table told the solver which [virtual packages](https://conda.io/projects/conda/en/latest/user-guide/tasks/manage-virtual.html) (`__cuda`, `__glibc`, `__osx`, `__linux`, `__archspec`) to assume were available on the host. The same information now lives on the platform itself, declared as an inline-table entry on `workspace.platforms`. This page shows the equivalent forms for the patterns that used to live under `[system-requirements]`.

## Why the change

Putting the constraints on the platform makes the data flow obvious:

- The solver knows up-front which virtual packages apply to which conda subdir, so the same workspace can mix CUDA-enabled and CUDA-free builds for the same subdir without juggling features.
- Features bind to a rich platform by *name* rather than by replaying the same set of virtual packages. Two features that pick the same platform can never declare conflicting versions of `__cuda`.
- The CLI ([`pixi workspace platform`](../../reference/cli/pixi/workspace/platform/)) has a single surface for declaring, editing, and removing these constraints.

## Equivalent forms

Before

```toml
[workspace]
platforms = ["linux-64"]

[system-requirements]
cuda = "12"
```

After

```toml
[workspace]
platforms = [
  { platform = "linux-64", cuda = "12" },
]
```

Before

```toml
[workspace]
platforms = ["linux-64", "osx-arm64"]

[system-requirements]
libc = { family = "glibc", version = "2.28" }
macos = "13.0"
```

After

```toml
[workspace]
platforms = [
  { platform = "linux-64", glibc = "2.28" },
  { platform = "osx-arm64", macos = "13.0" },
]
```

Before

```toml
[workspace]
platforms = ["linux-64"]

[feature.gpu.system-requirements]
cuda = "12"

[environments]
gpu = ["gpu"]
```

After

```toml
[workspace]
platforms = [
  "linux-64",
  { name = "linux-64-cuda", platform = "linux-64", cuda = "12" },
]

[feature.gpu]
platforms = ["linux-64-cuda"]

[environments]
gpu = ["gpu"]
```

The recognised friendly keys (`cuda`, `archspec`, `glibc`, `linux`, `macos`, `windows`) and the raw `__name = "version"` escape hatch are documented under [Declaring virtual packages per platform](../multi_platform_configuration/#declaring-virtual-packages-per-platform).

## CLI migration

| Old (deprecated, hidden)                            | New                                                   |
| --------------------------------------------------- | ----------------------------------------------------- |
| `pixi workspace system-requirements add cuda 12`    | `pixi workspace platform add linux-64 --cuda 12`      |
| `pixi workspace system-requirements add macos 13.5` | `pixi workspace platform edit osx-arm64 --macos 13.5` |
| `pixi workspace system-requirements list`           | `pixi workspace platform list`                        |

The `pixi workspace platform` subcommand keeps `pixi.lock` in sync when it mutates a rich entry.

## Default declared virtual packages

When you write a bare-string entry like `"linux-64"`, Pixi uses these defaults (matching what `[system-requirements]` used to default to):

`__linux = "4.18"`, `__glibc = "2.28"`

No defaults.

`__osx = "13.0"`

`__osx = "13.0"`

Override them by switching to an inline-table entry with the relevant keys.

## Environment-variable overrides

These overrides come from conda itself and apply regardless of how the platform is declared in `pixi.toml`. Use them when you need to install in an environment that doesn't match the declared virtual packages (for example a CPU-only CI runner solving a CUDA-enabled lock file).

- `CONDA_OVERRIDE_CUDA` sets the `__cuda` version. Example: `CONDA_OVERRIDE_CUDA=11`.
- `CONDA_OVERRIDE_CUDA_ARCH` sets the `__cuda_arch` compute capability, formatted as `{major}.{minor}`. Example: `CONDA_OVERRIDE_CUDA_ARCH=8.6`.
- `CONDA_OVERRIDE_GLIBC` sets the `__glibc` version. Example: `CONDA_OVERRIDE_GLIBC=2.28`.
- `CONDA_OVERRIDE_OSX` sets the `__osx` version. Example: `CONDA_OVERRIDE_OSX=13.0`.
- `CONDA_OVERRIDE_LINUX` sets the `__linux` version. Example: `CONDA_OVERRIDE_LINUX=4.18`.
- `CONDA_OVERRIDE_WIN` sets the `__win` version. Example: `CONDA_OVERRIDE_WIN=0`.
- `CONDA_OVERRIDE_ARCHSPEC` sets the `__archspec` CPU microarchitecture name. Example: `CONDA_OVERRIDE_ARCHSPEC=skylake`. Set it to `0` to mark the microarchitecture as unknown.

Setting any of these to an empty string (for example `CONDA_OVERRIDE_CUDA=""`) disables the corresponding virtual package instead of setting a version, which is useful for solving a lock file as if the feature were absent.

## Additional resources

For background on virtual packages in the conda ecosystem, see the [Conda documentation](https://docs.conda.io/projects/conda/en/latest/user-guide/tasks/manage-virtual.html).
