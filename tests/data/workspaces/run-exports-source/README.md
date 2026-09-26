# run-exports-source

Regression workspace for prefix-dev/pixi#6482: run dependencies introduced
purely via run-exports of source-built packages must be registered as
*source* dependencies of the consuming record; otherwise the top-level solve
looks for a binary package in the (empty) channels and fails.

Two shapes are covered:

- `package_a` host-depends on source `package_b`, which run-exports itself.
  The exported package is present in `package_a`'s host env, so its pinned
  source location is known there.
- `package_b` also run-exports `package_c`, which it host-depends on but
  which never appears in `package_a`'s host env. The source link comes from
  `package_b`'s own record `sources` map instead (this mirrors a recipe
  sibling output, like `python` weak-exporting `python_abi`).

The test configures the in-memory passthrough backend with the run-exports
for `package_b` (`PassthroughBackendInstantiator::with_run_exports`); the
backend emits the `package_c` export as a source spec because the project
model declares it as a source dependency.
