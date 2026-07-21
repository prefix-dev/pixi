# Standalone scripts

`pixi script` gives a standalone file its own environment. The requirements
stay with the file, while the environment remains separate from Pixi workspaces
and activated environments.

Pixi supports Python scripts through [PEP 723 inline
metadata](https://packaging.python.org/en/latest/specifications/inline-script-metadata/).
The commands on this page operate on that metadata.

## Make a Python script self-contained

This script downloads the USGS earthquake feed with `httpx`, then uses GDAL's
Python bindings to count the features:

```python title="earthquakes.py"
import httpx
from osgeo import ogr

ogr.UseExceptions()

response = httpx.get(
    "https://earthquake.usgs.gov/earthquakes/feed/v1.0/summary/all_hour.geojson"
)
dataset = ogr.Open(response.text)
print(f"{dataset.GetLayer().GetFeatureCount()} earthquakes in the past hour")
```

Running it normally requires both `httpx` and GDAL to be installed. Initialize
the script, add GDAL from Conda, and add `httpx` from PyPI so its requirement
is stored in the standard PEP 723 field:

```console
$ pixi script init earthquakes.py
$ pixi script add earthquakes.py gdal
$ pixi script add --pypi earthquakes.py httpx
$ pixi script run earthquakes.py
6 earthquakes in the past hour
```

Pixi preserves the Python code and adds a metadata block above it. The relevant
part looks like this:

```python title="earthquakes.py"
# /// script
# requires-python = ">=3.11"
# dependencies = ["httpx"]
#
# [tool.pixi.workspace]
# channels = ["conda-forge"]
#
# [tool.pixi.dependencies]
# gdal = "*"
# ///
```

The file now describes both sides of its environment. Anyone with Pixi can run
it without installing `httpx` or GDAL globally.

## Manage dependencies

A script can use packages from both Conda and PyPI. Many Python packages are
available from both, so the choice also determines how the requirement is
stored:

| Command | Metadata | Supported by |
| --- | --- | --- |
| `pixi script add earthquakes.py gdal` | `tool.pixi.dependencies` | Pixi |
| `pixi script add --pypi earthquakes.py httpx` | PEP 723 `dependencies` | Pixi and other PEP 723 tools |

`--pypi` puts the requirement in the standard PEP 723 `dependencies` field,
which tells Pixi to install it from PyPI and allows other PEP 723 tools such as
uv to read it. Without the flag, Pixi records a Conda dependency in
`tool.pixi.dependencies`. This Pixi-specific metadata uses a
[subset of the Pixi manifest](../reference/pixi_manifest.md), allowing richer
configuration, and a script can combine both forms.

Remove packages with the same source selection:

```console
$ pixi script remove earthquakes.py gdal
$ pixi script remove --pypi earthquakes.py httpx
```

Adding or removing a package updates the script and its environment. It does
not create a lock file unless the script already has one.

## Run the script

Pixi creates or reuses the script's environment, then runs the file with its
declared Python version and dependencies:

```console
$ pixi script run earthquakes.py
```

Dependencies from the current workspace or an activated environment are not
added to the script's environment.

Arguments after the script path are passed to Python:

```console
$ pixi script run process.py input.csv --verbose
```

Place Pixi options before the path:

```console
$ pixi script run --frozen earthquakes.py
```

Relative paths in the metadata are resolved from the script's directory. The
script itself runs in the directory where you invoked Pixi.

## Share the script

PEP 723 defines a portable core for Python scripts. Tools such as Pixi and uv
can read its standard fields:

- `requires-python` selects a compatible Python version.
- `dependencies` lists packages installed from PyPI.

Pixi extends that core with `tool.pixi`:

- `tool.pixi.workspace` configures channels and platforms.
- `tool.pixi.dependencies` lists Conda packages.
- `tool.pixi.pypi-dependencies` lists PyPI packages using Pixi's dependency
  syntax.

These fields can describe more of the environment than standard PEP 723
metadata, but they are only used by Pixi. In the example above, another PEP 723
tool can install `httpx`, but only Pixi also provides GDAL.

The fields behave as they do in a Pixi-enabled `pyproject.toml`. If both
`requires-python` and `tool.pixi.dependencies.python` are present, the explicit
Conda dependency determines the Python version.

Pixi ignores metadata for other tools and preserves it when editing the script.

## Lock exact versions

A lock file is optional. Without one, Pixi resolves the metadata and reuses the
environment from its cache.

Create a sidecar lock when you want to preserve the exact package resolution:

```console
$ pixi script lock earthquakes.py
```

Pixi writes `earthquakes.py.pixi.lock` next to the script. Once it exists, `run`,
`add`, `remove`, and `lock` keep it up to date.

The first lock includes the current platform. To lock for specific platforms,
repeat `--platform`:

```console
$ pixi script lock --platform linux-64 --platform osx-arm64 earthquakes.py
```

Later commands reuse the platforms in the lock unless the script declares
`tool.pixi.workspace.platforms` or you replace them with another
`script lock --platform` invocation.

Use `--locked` to require an existing, up-to-date lock. Use `--frozen` to run
from an existing lock without checking whether the metadata has changed.

## Configure Pixi

Most scripts only need channels and dependencies. For more control,
`tool.pixi` accepts a focused set of environment settings from the
[Pixi manifest](../reference/pixi_manifest.md), using the same syntax for
platforms, constraints, activation, target-specific dependencies, and resolver
options. Because a script has one environment, workspace orchestration and
package or build configuration are not supported.
