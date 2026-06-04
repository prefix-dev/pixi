# Handoff: workspace-level `[cache.pypi-mapping]` is ignored (issue #6281)

## Context

Issue #6281 has two distinct bugs. The first (a misleading warning that
suggested the wrong config key, `cache.conda-pypi-mapping` instead of
`cache.pypi-mapping`) is already fixed on branch
`claude/clever-hawking-BJ9tZ`. This handoff covers the second, still-open
bug.

## The problem

The conda-pypi-mapping cache path is resolved through a global-only code
path that reads only the system and user `config.toml`. It never consults
the workspace-merged config. So a `[cache.pypi-mapping]` setting placed in
a workspace `.pixi/config.toml` is silently ignored, the network-filesystem
redirect still happens, and the `cache for PypiMapping ... is on a
network/parallel filesystem` warning keeps firing.

This is exactly what the reporter hit: they set both `repodata` and
`pypi-mapping` in their workspace `.pixi/config.toml`. The `repodata`
warning went away (repodata resolves through the workspace-aware path), but
the `PypiMapping` warning persisted. Only `PIXI_CACHE_DIR` (an env var read
by the global path) made it stop.

## Root cause

There are two ways a cache path gets resolved:

- Workspace-aware: `Config::cache_dir_for(&self, kind)` at
  `crates/pixi_config/src/lib.rs:2710`. This respects workspace-level
  `[cache.*]` overrides. Used by repodata and by the uv wheel cache
  (`crates/pixi_uv_context/src/lib.rs:156`).
- Global-only: the free function `cache_dir_for(kind)` at
  `crates/pixi_config/src/lib.rs:608`, which reads `GLOBAL_CACHE_CONFIG`
  (`Config::load_global()`, system + user config only).

The mapping client uses the global-only one. In
`crates/pypi_mapping/src/lib.rs:182`, inside `MappingClient::builder`, the
cache path is resolved with the free `cache_dir_for(CacheKind::PypiMapping)`.
That path is baked into the HTTP cache strategy and the wrapped client
closure constructed inside `builder()`, so it cannot be patched after the
fact with a simple setter.

## Suggested fix

Make the mapping cache path come from the workspace-aware config and pass
it into the builder.

1. Change `MappingClient::builder` in `crates/pypi_mapping/src/lib.rs` to
   accept the resolved cache path as a parameter, e.g.
   `builder(client: LazyClient, cache_path: PathBuf)`, and drop the internal
   `cache_dir_for(CacheKind::PypiMapping)` call at line 182. This keeps the
   `pypi_mapping` crate free of any opinion about which config layer wins.

2. At the real call site,
   `crates/pixi_core/src/lock_file/update.rs:1796-1800`, resolve the path
   from the workspace config (the `project` value is in scope):
   `project.config().cache_dir_for(pixi_config::CacheKind::PypiMapping)`.
   Pass the result into `builder(...)`.

3. Update the test call sites in
   `crates/pixi/tests/integration_rust/solve_group_tests.rs` (many
   `MappingClient::builder(client.clone())` calls). They can pass a temp dir
   or the global `pixi_config::cache_dir_for(CacheKind::PypiMapping)` result
   to preserve current behavior.

Keep the `.expect(...)` error message behavior reasonable wherever the path
is resolved.

## How to verify

- Set `[cache] pypi-mapping = "/tmp/whatever/conda-pypi-mapping"` in a
  workspace `.pixi/config.toml` on a network filesystem (or with the netfs
  redirect forced) and confirm the `cache for PypiMapping ...` warning no
  longer fires and the mapping cache is written to the configured path.
- Add a regression test mirroring the existing
  `cache_dir_for_per_kind_path_override_wins` style in
  `crates/pixi_config/src/lib.rs`, but exercising the mapping client's path
  selection through a workspace `Config`.

## Branch

Continue on `claude/clever-hawking-BJ9tZ` (the warning-text fix is already
there).
