# Conda-PyPI integration

This example shows how to use dependencies from the [conda-pypi channel](https://conda.anaconda.org/conda-pypi) alongside PyPI dependencies.

- Python comes from conda-forge.
- `boltons` is explicitly sourced from conda-pypi as a wheel, converted into the
  conda package cache by rattler, and installed by the conda installer.
- `flask` uses `[pypi-dependencies]` and is installed through uv as usual.

The example currently targets macOS ARM64 and Python 3.12. The lockfile records
boltons under `conda:` with a `.whl` URL, while Flask is recorded under `pypi:`.

From the repository root, use the binary built from this branch:

```sh
cargo build --locked -p pixi
./target/debug/pixi run --locked \
  --manifest-path examples/conda-pypi-integration/pixi.toml test
```

The test exercises boltons and a Flask test-client request without starting a
server. Converted wheels use `pkgs/wheels-v1/` inside the package cache.

For a fresh environment and isolated cache:

```sh
TEST_DIR=$(mktemp -d)
cp examples/conda-pypi-integration/{pixi.toml,pixi.lock,check.py} "$TEST_DIR/"
PIXI_CACHE_DIR="$TEST_DIR/cache" ./target/debug/pixi run --locked \
  --manifest-path "$TEST_DIR/pixi.toml" test
```

**Draft status:** building against the released wheel-support crates currently
requires a compatible rattler-build release. Version 0.2.21 still pulls an older
cache with an incompatible exact networking dependency. The repository Cargo
lockfile must be refreshed and the release-based build validated once resolved.
The example lockfile was generated with the earlier local integration build.
