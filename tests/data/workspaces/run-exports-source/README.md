# run-exports-source

Regression workspace for prefix-dev/pixi#6482: the top-level environment
depends on source `package_a`, whose host env contains source `package_b`.
`package_b` declares a strong run-export on itself, so `package_a`'s
assembled record gains a run dependency on `package_b` that is introduced
purely via run-exports. That dependency must be registered as a *source*
dependency of `package_a`'s record; otherwise the top-level solve looks for
a binary `package_b` in the (empty) channels and fails.

The test configures the in-memory passthrough backend with the strong
run-export for `package_b` (`PassthroughBackendInstantiator::with_run_exports`).
