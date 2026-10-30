# conda-pypi integration

This example uses packages from the [conda-pypi](https://github.com/conda/conda-pypi) channel together with conda-forge and PyPI dependencies.

- `python` comes from conda-forge.
- `boltons` and `black` are wheels from the conda-pypi channel, installed as conda packages.
- `flask` is a regular `[pypi-dependencies]` entry installed through uv.

Run the smoke test with:

```sh
pixi run test
```
