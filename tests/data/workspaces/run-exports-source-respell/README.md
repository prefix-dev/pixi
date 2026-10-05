# run-exports-source-respell

Regression workspace for source dependencies that reach a record through two
channels with different spellings of the same location:

- `package_b` run-exports itself, so `package_a`'s run dependencies receive
  an *implied* source spec carrying the pinned location (`../package_b`
  after relativizing against `package_a`'s manifest).
- `package_a`'s own run-exports also name `package_b`; the backend emits it
  as a source spec with the *manifest* spelling, which is deliberately
  written as `./../package_b` here.

The explicit manifest spelling must win over the implied pinned location
instead of the record assembly failing with a `DuplicateSourceDependency`
error. The same shape arises with git sources, where the pinned location is
a commit while the manifest names a branch.
