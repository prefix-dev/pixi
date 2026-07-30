# Standalone Python scripts

Pixi can give a single Python file its own environment. Dependencies and
configuration live in a [PEP 723 inline metadata
block](https://packaging.python.org/en/latest/specifications/inline-script-metadata/),
while the resolved environment stays in Pixi's cache instead of a workspace
next to the script.

Script commands use `--script <PATH>`. The same `init`, `run`, `add`, `remove`,
and `lock` commands used for workspaces can therefore operate on either a
manifest or a standalone file.

## Make a script self-contained

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

Initialize the metadata, add GDAL from Conda, add `httpx` from PyPI, and run
the file:

```console
pixi init --script earthquakes.py --channel conda-forge
pixi add --script earthquakes.py gdal
pixi add --script earthquakes.py --pypi httpx
pixi run --script earthquakes.py
6 earthquakes in the past hour
```

Pixi preserves the Python source and writes the metadata into the file:

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

The script now describes both sides of its environment. It can be copied
without a separate `pixi.toml`, `pyproject.toml`, or lock file.

## Portable and Pixi-specific metadata

PEP 723 defines two portable fields:

- `requires-python` selects a compatible Python version.
- `dependencies` lists packages installed from PyPI.

Pixi reads those fields and extends them with a focused subset of
`tool.pixi`:

- `tool.pixi.workspace` configures channels, platforms, and resolver options.
- `tool.pixi.dependencies` lists Conda packages.
- `tool.pixi.pypi-dependencies` represents PyPI requirements that need
  Pixi-specific fields, such as an index or editable installation.
- `tool.pixi.target.<platform>` holds platform-specific dependencies,
  constraints, and activation settings.

Other PEP 723 tools can use the portable fields and ignore `tool.pixi`. In the
example above, another tool can install `httpx`, but only Pixi also provides
GDAL. Pixi preserves metadata under other `tool.*` tables when editing a
script.

An explicit `tool.pixi.dependencies.python` requirement takes precedence over
`requires-python` when both are present.

## Manage dependencies

Use `--pypi` to choose PyPI. Without it, `add` and `remove` operate on Conda
dependencies:

```console
pixi add --script earthquakes.py gdal
pixi add --script earthquakes.py --pypi "httpx>=0.28"
pixi remove --script earthquakes.py gdal
pixi remove --script earthquakes.py --pypi httpx
```

Requirements that standard PEP 723 can express remain in `dependencies`.
Richer requirements are written under `tool.pixi`:

```console
# Add a dependency only for a declared platform.
pixi add --script analysis.py --platform linux-64 libblas

# Preserve a dependency-specific package index.
pixi add --script analysis.py --pypi \
    --index https://pypi.example.com/simple "internal-api>=2"

# Install a local Python project in editable mode.
pixi add --script analysis.py --pypi --editable \
    "analysis-tools @ ./analysis-tools"
```

Relative paths in metadata are resolved from the script's directory.

When no adjacent lock file exists, dependency mutations solve for validation
but write only the inline metadata. If a sidecar lock already exists, it is
updated as part of the mutation.

## Run the script

`pixi run --script` creates or reuses the cached environment, then invokes the
file with its declared Python and dependencies:

```console
pixi run --script earthquakes.py
```

`run` also accepts a direct HTTP or HTTPS URL, including URLs without a
`.py` suffix:

```console
pixi run --script https://example.com/earthquakes.py
pixi run --script https://gist.github.com/user/gist-id
```

Remote scripts must already contain a PEP 723 metadata block. They are fetched
on every invocation and executed from a secure temporary `.py` file, while
their environment is reused from Pixi's cache. Relative paths in remote
metadata resolve from the directory where Pixi was invoked.

Remote inputs are execution-only: commands that edit, inspect, export, or lock
a script continue to require a local path. A remote script has no adjacent lock
file, so it cannot be run with `--locked` or `--frozen`.

For a normal GitHub Gist page, Pixi selects the first filename ending in
`.py`, case-insensitively, or the first file when the Gist contains no Python
filename. Set `PIXI_GITHUB_TOKEN` to authenticate the Gist API request, for
example when accessing a private Gist. Pixi sends this token only to
`api.github.com`, never to the selected file's raw URL.

> A remote script executes with your user permissions. Inspect the source or
> trust its publisher before running it.

Arguments after Pixi's options are forwarded to the script:

```console
pixi run --script process.py input.csv --verbose
```

Put Pixi options before the forwarded arguments:

```console
pixi run --frozen --script earthquakes.py
```

The environment is isolated from an enclosing Pixi workspace and from any
currently activated environment. The script itself runs with the directory
from which Pixi was invoked as its working directory.

## Inspect and export

Inspect the resolved dependency graph with `tree`:

```console
pixi tree --script earthquakes.py
pixi tree --script earthquakes.py httpx
```

Export the default script environment in either supported Conda format:

```console
pixi workspace export conda-environment --script earthquakes.py
pixi workspace export conda-explicit-spec --script earthquakes.py ./export
```

`conda-environment` renders from the manifest by default. Pass
`--from-lock-file` to export exact versions from an adjacent lock.
`conda-explicit-spec` resolves packages when necessary and accepts the normal
lock policy flags.

## Lock exact versions

A lock file is optional. Without one, commands resolve the script in memory
and reuse Pixi's caches:

```console
pixi run --script earthquakes.py
```

Create a sidecar when exact versions need to travel with the script:

```console
pixi lock --script earthquakes.py
```

This writes `earthquakes.py.pixi.lock` next to the file. Subsequent commands
reuse the sidecar, and metadata mutations keep it up to date.

To lock more than the current platform, declare the platforms first:

```console
pixi workspace platform add --script earthquakes.py linux-64 osx-arm64
pixi lock --script earthquakes.py
```

Use `--locked` to require an existing, up-to-date sidecar. Use `--frozen` to
consume an existing sidecar without updating it. Both options report an error
when the script has no adjacent lock file.

## Manage channels and platforms

Scripts have one implicit environment, but they can declare multiple channels
and target platforms.

Manage channels through `pixi workspace channel`:

```console
pixi workspace channel add --script analysis.py conda-forge
pixi workspace channel list --script analysis.py
pixi workspace channel remove --script analysis.py conda-forge
```

Manage platforms through `pixi workspace platform`:

```console
pixi workspace platform add --script analysis.py linux-64 osx-arm64
pixi workspace platform list --script analysis.py
pixi workspace platform remove --script analysis.py osx-arm64
```

The platform command also supports rich platform definitions and the `edit`
and `move` operations. Platform order determines selection priority. Declared
platforms are also the platforms consumed by `pixi lock --script`; the lock
command does not take a separate platform override.

## Supported command surface

The script-capable commands are:

| Operation | Command |
| --- | --- |
| Initialize metadata | `pixi init --script <PATH>` |
| Run | `pixi run --script <PATH>` |
| Add or remove dependencies | `pixi add --script <PATH>`, `pixi remove --script <PATH>` |
| Create or update a lock | `pixi lock --script <PATH>` |
| Inspect dependencies | `pixi tree --script <PATH>` |
| Manage channels | `pixi workspace channel ... --script <PATH>` |
| Manage platforms | `pixi workspace platform ... --script <PATH>` |
| Export | `pixi workspace export ... --script <PATH>` |

A script has no workspace name, version, registration, named environments,
features, or tasks. Commands and options that manage those concepts reject
`--script`. Persistent workspace operations such as install, shell, update,
and configuration also remain workspace-only.
