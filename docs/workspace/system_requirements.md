!!! warning "`[system-requirements]` is deprecated"
    Declare these constraints directly on `[workspace].platforms` instead --
    see [Declaring virtual packages per platform](./multi_platform_configuration.md#declaring-virtual-packages-per-platform)
    for the inline-table syntax and the matching
    [`pixi workspace platform`](../reference/cli/pixi/workspace/platform/index.md) CLI.
    Existing `[system-requirements]` tables are still parsed and migrated
    transparently, so older manifests keep working, but new manifests should
    use the per-platform form.

# Migrating from `[system-requirements]`

The `[system-requirements]` table told the solver which [virtual
packages](https://conda.io/projects/conda/en/latest/user-guide/tasks/manage-virtual.html)
(`__cuda`, `__glibc`, `__osx`, `__linux`, `__archspec`) to assume were available
on the solve target. The same information now lives on the platform itself, declared as
an inline-table entry on `workspace.platforms`. This page shows the equivalent
forms for the patterns that used to live under `[system-requirements]`.

## Why the change

Putting the constraints on the platform makes the data flow obvious:

- The solver knows up-front which virtual packages apply to which conda subdir,
  so the same workspace can mix CUDA-enabled and CUDA-free builds for the same
  subdir without juggling features.
- Features bind to a platform by *name* rather than by replaying the same
  set of virtual packages. Two features that pick the same platform can never
  declare conflicting versions of `__cuda`.
- The CLI ([`pixi workspace platform`](../reference/cli/pixi/workspace/platform/index.md))
  has a single surface for declaring, editing, and removing these constraints.

## Equivalent forms

=== "Workspace-level CUDA"
    ```toml title="Before"
    [workspace]
    platforms = ["linux-64"]

    [system-requirements]
    cuda = "12"
    ```

    ```toml title="After"
    [workspace]
    platforms = [
      { platform = "linux-64", cuda = "12" },
    ]
    ```

=== "Workspace-level libc / macOS"
    ```toml title="Before"
    [workspace]
    platforms = ["linux-64", "osx-arm64"]

    [system-requirements]
    libc = { family = "glibc", version = "2.28" }
    macos = "13.0"
    ```

    ```toml title="After"
    [workspace]
    platforms = [
      { platform = "linux-64", glibc = "2.28" },
      { platform = "osx-arm64", macos = "13.0" },
    ]
    ```

=== "Per-feature CUDA"
    ```toml title="Before"
    [workspace]
    platforms = ["linux-64"]

    [feature.gpu.system-requirements]
    cuda = "12"

    [environments]
    gpu = ["gpu"]
    ```

    ```toml title="After"
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

The recognised friendly keys (`cuda`, `archspec`, `glibc`, `linux`, `macos`,
`windows`) and the raw `__name = "version"` escape hatch are documented under
[Declaring virtual packages per platform](./multi_platform_configuration.md#declaring-virtual-packages-per-platform).

## CLI migration

| Old (deprecated, hidden) | New |
|--------------------------|-----|
| `pixi workspace system-requirements add cuda 12` | `pixi workspace platform add linux-64 --cuda 12` |
| `pixi workspace system-requirements add macos 13.5` | `pixi workspace platform edit osx-arm64 --macos 13.5` |
| `pixi workspace system-requirements list` | `pixi workspace platform list` |

The `pixi workspace platform` subcommand keeps `pixi.lock` in sync when it
edits a platform.

## Default declared virtual packages

See [Default declared virtual packages](./multi_platform_configuration.md#default-declared-virtual-packages) for the defaults applied to bare subdirs and inline-table entries.

## Environment-variable overrides

See [Environment-variable overrides](./multi_platform_configuration.md#environment-variable-overrides) for built-in and detector overrides.
Overrides change the host view used for compatibility checks, not the solve inputs of a declared workspace target.

## Additional resources

For background on virtual packages in the conda ecosystem, see the
[Conda documentation](https://docs.conda.io/projects/conda/en/latest/user-guide/tasks/manage-virtual.html).
