A source dependency normally points at a directory or repository that contains
its own `pixi.toml` with a `[package]` section.
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
rust-package = { git = "https://github.com/user/repo.git", package = { build = { backend = { name = "pixi-build-rust" } } } }
```

The dotted-key form is often shorter:

```toml title="pixi.toml"
[dependencies]
rust-package.git = "https://github.com/user/repo.git"
rust-package.package.build.backend.name = "pixi-build-rust"
```

Pixi builds `rust-package` from the given source using the inline definition,
without looking for a `pixi.toml` in the repository.

## The source comes from the dependency

The source location stays on the dependency itself through the usual `git`,
`path`, or `url` fields. You do not repeat it inside `package.build.source`;
setting `package.build.source` on an inline definition is an error.

A `path` source is resolved relative to the manifest that declares the
dependency:

```toml title="pixi.toml"
[dependencies]
my-lib = { path = "vendor/my-lib", package.build.backend.name = "pixi-build-cmake" }
```

## What you can define inline

An inline definition accepts the same fields as a standalone `[package]`
section. Besides `build`, you can declare the package's own version,
dependencies, and so on:

```toml title="pixi.toml"
[dependencies]
my-lib = { git = "https://github.com/user/repo.git", package = { version = "1.2.3", build = { backend = { name = "pixi-build-cmake" } }, run-dependencies = { fmt = ">=10" } } }
```

Two fields behave differently from a standalone manifest:

- `name` is taken from the dependency key, so it cannot be set inline.
- `version` is optional; the build backend provides one when it is omitted.

## Where inline definitions are allowed

Inline definitions are accepted wherever a source dependency is, namely
`[dependencies]`, `[host-dependencies]`, `[build-dependencies]`, and their
`[feature.*]` and `[target.*]` variants.
