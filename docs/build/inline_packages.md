A source dependency normally points at a directory or repository that contains
its own Pixi manifest with a `[package]` section.
Pixi reads that manifest to learn how to build the package.

For small packages, or for code in another repository that does not ship a
`pixi.toml`, writing a separate manifest is more work than the package is worth.
An inline package definition lets you describe the package directly on the
dependency instead.

!!! warning
    Inline package definitions are part of the `pixi-build` preview and will
    change until it is stabilized. Add `"pixi-build"` to `workspace.preview` to
    use them.

## Defining a package inline

Add a `package` table to the source dependency. The build definition goes under
`package.build`, exactly as it would in a standalone `pixi.toml`:

```toml title="pixi.toml"
[dependencies]
rust-package = { git = "https://github.com/user/repo.git", package.build.backend.name = "pixi-build-rust" }
```

Pixi builds `rust-package` from the given source using the inline definition,
without looking for a `pixi.toml` in the repository.

## The source comes from the dependency

However, the allowed keys are not exactly the same as with `[package]`.
For once, the source location stays on the dependency itself through the usual `git`,
`path`, or `url` fields. You do not repeat it inside `package.build.source`;
setting `package.build.source` on an inline definition is an error.

A `path` source is resolved relative to the manifest that declares the
dependency:

```toml title="pixi.toml"
[dependencies]
my-lib = { path = "vendor/my-lib", package.build.backend.name = "pixi-build-cmake" }
```

Also, you don't specify `package.name`.
Pixi can infer from your dependency declaration that the name has to be `my-lib`.

## What you can define inline

Apart from that, inline definition accepts the same fields as a standalone `[package]`
section.
Besides `build`, you can declare the package's own version, dependencies, and so on:

```toml title="pixi.toml"
[dependencies]
my-lib = { git = "https://github.com/user/repo.git", package = { version = "1.2.3", build = { backend = { name = "pixi-build-cmake" } }, run-dependencies = { fmt = ">=10" } } }
```

## Where inline definitions are allowed

Inline definitions are accepted wherever a source dependency is:

- `[dependencies]`, `[host-dependencies]` and `[build-dependencies]`, and their
  `[feature.*]` and `[target.*]` variants,
- the package dependency tables `[package.run-dependencies]`,
  `[package.host-dependencies]`, `[package.build-dependencies]` and
  `[package.extra-dependencies.*]`, including their `if(...)` conditional
  sub-tables,
- the `[workspace.dependencies]` pool (see below).

This works in *any* package manifest Pixi builds — not just your workspace's
own `[package]` section, but also the manifests of `path`, `git` or `url`
source dependencies. Inline definitions also nest: a definition's own
dependency tables may declare further inline definitions, so a chain of
manifest-less repositories can be described from one place.

They are not accepted in `[constraints]` or `[package.run-constraints]`;
constraints only apply to packages resolved from channels.

## Inheriting a definition through the workspace pool

An entry in `[workspace.dependencies]` may carry an inline definition. A
package dependency that opts in with `{ workspace = true }` inherits the
source location *and* the definition together, so the package is declared
once and used by every member:

```toml title="pixi.toml"
[workspace.dependencies]
rust-package = { git = "https://github.com/user/repo.git", package.build.backend.name = "pixi-build-rust" }

[package.run-dependencies]
rust-package = { workspace = true }
```

Combining `workspace = true` with a `package` table at the use site is an
error; declare the definition on the pool entry instead.

## Conflicting definitions

Inline definitions are matched to dependencies by package name. Within one
dependency table set, a name may carry at most one definition. When the same
package (at the same source location) is reached through several declarers
during a solve:

- a definition in *your* manifest's dependency tables overrides whatever a
  transitive package declares, just like it overrides an on-disk manifest;
- two transitive packages that disagree about the definition are an error —
  there is no priority order between arbitrary packages. Resolve it by
  declaring the dependency (with one definition) at the workspace level. 