# Artifact-cache input reproducer

This CMake package installs a marker file,
`share/artifact-key-repro/repro-value.txt`, written from `REPRO_VALUE` during
CMake configuration. It checks both artifact reuse and
reuse of an already-configured backend build directory.

Copy this fixture to a fresh directory and run:

```sh
pixi install --manifest-path /path/to/copy/pixi.toml
```

The installed package should be version `0.1.0`, and the file under
`.pixi/envs/default/share/artifact-key-repro/repro-value.txt` should contain
`before`. The installed version is recorded in
`.pixi/envs/default/conda-meta/artifact-key-repro-*.json`.

Change `REPRO_VALUE` in `pixi.toml` to `after` and rerun the same command without
clearing `.pixi`. The installed file must now contain `after`.

Then change the package version to `0.2.0` and rerun. The installed package record
must now report `0.2.0`. An unchanged invocation should reuse the package.

On installed Pixi 0.81.0 on macOS arm64, both changed installs reported success
but retained the `0.1.0` package and `before` marker file contents.

The automated regression is
`test_artifact_cache_tracks_package_and_build_settings` in
`tests/integration_python/pixi_build/test_config.py`. It tests version, general
configuration, and target-specific configuration changes independently, then
checks an unchanged invocation:

```sh
pixi run test-specific-test-debug artifact_cache_tracks_package_and_build_settings
```
