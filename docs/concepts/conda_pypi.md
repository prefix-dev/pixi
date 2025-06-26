Pixi is built on top of both the conda and PyPI ecosystems.

**Conda** is a cross-platform, cross-language package ecosystem that allows users to install packages and manage environments.
It is widely used in the data science and machine learning community, but it is also used in other fields.
It’s power comes from the fact that it always installs binary packages, meaning that it doesn’t need to compile anything.
This makes the ecosystem very fast and easy to use.

**PyPI** is the Python Package Index, which is the main package index for Python packages.
It is a much larger ecosystem than conda, especially because the boundary to upload packages is lower.
This means that there are a lot of packages available, but it also means that the quality of the packages is not always as high as in the conda ecosystem.

Pixi can install packages from **both** **ecosystems**, but it uses a **conda-first approach**.

The simplified process is as follows:

1. Resolve the conda dependencies.
2. Map the conda packages to PyPI packages.
3. Resolve the remaining PyPI dependencies.


## Tool Comparison
Here is a non-exhaustive comparison of the features of conda and PyPI ecosystems.

| Feature | Conda | PyPI |
| ------- | ----- | ---- |
| Package format | Binary | Source & Binary (wheel) |
| Package managers | [`conda`](https://github.com/conda/conda), [`mamba`](https://github.com/mamba-org/mamba), [`micromamba`](https://github.com/mamba-org/mamba), [`pixi`](https://github.com/prefix-dev/pixi)  | [`pip`](https://github.com/pypa/pip), [`poetry`](https://github.com/python-poetry/poetry), [`uv`](https://github.com/astral-sh/uv), [`pdm`](https://github.com/pdm-project/pdm), [`hatch`](https://github.com/pypa/hatch), [`rye`](https://github.com/astral-sh/rye), [`pixi`](https://github.com/prefix-dev/pixi) |
| Environment management | [`conda`](https://github.com/conda/conda), [`mamba`](https://github.com/mamba-org/mamba), [`micromamba`](https://github.com/mamba-org/mamba), [`pixi`](https://github.com/prefix-dev/pixi) | [`venv`](https://docs.python.org/3/library/venv.html), [`virtualenv`](https://virtualenv.pypa.io/en/latest/), [`pipenv`](https://pipenv.pypa.io/en/latest/), [`pyenv`](https://github.com/pyenv/pyenv), [`uv`](https://github.com/astral-sh/uv), [`poetry`](https://github.com/python-poetry/poetry), [`pixi`](https://github.com/prefix-dev/pixi) |
| Package building | [`conda-build`](https://github.com/conda/conda-build), [`pixi`](https://github.com/prefix-dev/pixi) | [`setuptools`](https://github.com/pypa/setuptools), [`poetry`](https://github.com/python-poetry/poetry), [`flit`](https://github.com/pypa/flit), [`hatch`](https://github.com/pypa/hatch), [`uv`](https://github.com/astral-sh/uv), [`rye`](https://github.com/astral-sh/rye) |
| Package index | [`conda-forge`](https://prefix.dev/channels/conda-forge), [`bioconda`](https://prefix.dev/channels/bioconda), and more | [pypi.org](https://pypi.org) |

## `uv` by Astral
Pixi uses the [`uv`](https:://github.com/astral-sh/uv) library to handle PyPI packages.
Pixi doesn't install `uv` the tool itself, because both tools are build in Rust it is used as a library.

We're extremely grateful to the [Astral](https://astral.sh) team for their work on `uv`, which is a great library that allows us to handle PyPI packages in a much better way than before.

Initially, next to `pixi` were building a library called `rip` which had the same goals as `uv`, but we decided to switch to `uv` because it quickly became a more mature library, and it has a lot of features that we need.

- [Initial blogpost about announcing `rip`](https://prefix.dev/blog/pypi_support_in_pixi)
- [Blogpost to announce the switch to `uv`](https://prefix.dev/blog/uv_in_pixi)

## Solvers
Because Pixi supports both ecosystems, it currently needs two different solvers to handle the dependencies.

- The [`resolvo`](https://github.com/prefix-dev/resolvo) library is used to solve the conda dependencies. Implemented in [`rattler`](https://github.com/conda/rattler).
- The [`PubGrub`](https://github.com/pubgrub-rs/pubgrub) library is used to solve the PyPI dependencies. Implemented in [`uv`](https:://github.com/astral-sh/uv).

!!! Note
    The holy grail of Pixi is to have a single solver that can handle both ecosystems.
    Because resolvo is written to support both ecosystems, it is possible to use it for PyPI packages as well, but this is not yet implemented.

Because of these two solvers, we need to make a decision which solver to run first.
This is the conda-first approach, which means that we first solve the conda dependencies and then the PyPI dependencies.

Pixi first run the conda (`rattler`) solver, which will resolve the conda dependencies.
Then it maps the conda packages to PyPI packages, using [`parselmouth`](https://github.com/prefix-dev/parselmouth)
Then it runs the PyPI (`uv`) solver, which will resolve the remaining PyPI dependencies.

The consequence is that Pixi will install the `conda` package if both are available.

Here is an example of how this works in practice:
```toml title="pixi.toml"
[dependencies]
python = ">=3.8"
numpy = ">=1.21.0"

[pypi-dependencies]
numpy = ">=1.21.0"
```

Which results in the following output:
```output
➜ pixi list -x
Package  Version  Build               Size      Kind   Source
numpy    2.3.1                        43.8 MiB  pypi   numpy-2.3.1-cp313-cp313-macosx_11_0_arm64.whl
python   3.13.5   hf3f3da0_102_cp313  12.3 MiB  conda  https://conda.anaconda.org/conda-forge/
```

In this example, Pixi will first resolve the conda dependencies and install the `numpy` and `python` conda packages.
Then it will map the `numpy` conda package to the `numpy` PyPI package and resolve the remaining PyPI dependencies.
Which is non as it was already installed as a conda package. Thus, no additional PyPI packages will be installed.

Another example is when you have a PyPI package that is not available as a conda package:
```toml title="pixi.toml"
[dependencies]
python = ">=3.8"

[pypi-dependencies]
numpy = ">=1.21.0"
```
Which results in the following output:
```output
> pixi list --explicit
Package  Version  Build               Size      Kind   Source
numpy    2.3.1                        43.8 MiB  pypi   numpy-2.3.1-cp313-cp313-macosx_11_0_arm64.whl
python   3.13.5   hf3f3da0_102_cp313  12.3 MiB  conda  https://conda.anaconda.org/conda-forge/
```
In this example, Pixi will first resolve the conda dependencies and install the `python` conda package.
Then since `numpy` is not available as a conda package, it will resolve the PyPI dependencies and install the `numpy` PyPI package.

To override or change the mapping of conda packages to PyPI packages, you can use the [`conda-pypi-map`](../reference/pixi_manifest.md#conda-pypi-map-optional) field in the `pixi.toml` file.
