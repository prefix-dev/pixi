In a monorepo, packages typically share many of the same dependency versions:
the build backend, the language runtime, common libraries, sibling packages
referenced by relative path.
Bumping such a version requires editing every package's `pixi.toml`, and the
files drift out of sync when someone forgets one.

`[workspace.dependencies]` solves this by letting the workspace declare a pool
of dependency specs that packages opt into per entry.

!!! warning
    `[workspace.dependencies]` is part of the `pixi-build` preview and applies
    **only to package dependencies** (`[package.*-dependencies]`,
    `[package.run-constraints]` and `[package.build.backend]`, including their
    `[package.target.<sel>.*]` variants).
    The workspace-level environment tables (`[dependencies]`,
    `[host-dependencies]`, `[build-dependencies]`, `[pypi-dependencies]`,
    `[constraints]`) do **not** participate; entries there continue to be
    declared directly.

## Defining common dependencies

Add a `[workspace.dependencies]` table to the workspace manifest. Entries use
the same syntax as any other conda dependency table.

```toml title="pixi.toml (workspace root)"
[workspace]
name = "monorepo"
channels = ["https://prefix.dev/conda-forge"]
platforms = ["linux-64", "osx-arm64", "win-64"]
preview = ["pixi-build"]

[workspace.dependencies]
numpy = "1.*"
pixi-build-cmake = "0.3.*"
boltons = { version = ">=24", channel = "conda-forge" }
shared-lib = { path = "packages/shared-lib" }
```

Relative `path` specs are resolved against the workspace manifest's directory
and re-anchored automatically when handed to a package in a different
directory: from `packages/foo/pixi.toml` above, `shared-lib` will resolve to
`../shared-lib`.

## Using a workspace dependency in a package

A package opts in per entry by writing `{ workspace = true }` instead of a
direct spec. The dotted-key shorthand `name.workspace = true` is equivalent.

```toml title="packages/foo/pixi.toml"
[package]
name = "foo"
version = "0.1.0"

[package.build]
backend = { name = "pixi-build-cmake", workspace = true }

[package.host-dependencies]
numpy = { workspace = true }
shared-lib = { workspace = true }

[package.run-dependencies]
boltons.workspace = true
```

A name without `{ workspace = true }` is unaffected: the package keeps full
control over that entry.

The inheritance marker is recognized in every package dependency table:

- `[package.host-dependencies]`
- `[package.build-dependencies]`
- `[package.run-dependencies]`
- `[package.run-constraints]`
- `[package.target.<selector>.*]` variants of the above
- `[package.build.backend]` (the lookup key is the backend's `name`)

## Layering package overrides

A package can add fields alongside `workspace = true` to override or extend
the workspace entry. Any field on a
[conda spec table](../reference/pixi_manifest.md#dependencies) may be
overridden, **except `version`**, which is mutually exclusive with
`workspace`; if the package needs a different version, drop the inheritance
marker and write a direct spec. When both sides set the same field, the
package's value wins.

```toml
[package.host-dependencies]
# Use the workspace's numpy version, but pin a specific build string here.
numpy = { workspace = true, build = "py311*" }
```
