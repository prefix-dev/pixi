# conda-pypi wheels

Solves `cowsay` from the [conda-pypi](https://conda.anaconda.org/conda-pypi) channel,
which serves PyPI wheels as repodata v3 `whl` records. The package is pinned to that
channel because `cowsay` also exists on conda-forge and would otherwise win under strict
channel priority.

A locked wheel appears in `pixi.lock` as an ordinary `conda:` entry whose location is the
wheel file on `files.pythonhosted.org`, with the conda-style identifier in `file_name`:

```yaml
- conda: https://files.pythonhosted.org/packages/.../cowsay-6.1-py3-none-any.whl
  name: cowsay
  version: '6.1'
  build: py3_none_any_0
  file_name: cowsay-6.1-py3_none_any_0.whl
```

Status (2026-10-17): `pixi lock` currently fails on this project with
`encountered duplicate records for typing_extensions-*.whl`, because the channel publishes
both a `typing-extensions` and a `typing_extensions` shard with identical records. Installing
a locked wheel also needs the rattler wheel fetch and link work. Both are tracked under the
conda-pypi integration (PFX-2080).
