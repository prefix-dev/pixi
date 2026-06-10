# Inline Script Metadata Examples

This directory contains examples demonstrating inline script metadata with
[`pixi exec`](https://pixi.sh/latest/reference/cli/pixi/exec/).

## What is inline script metadata?

A [PEP 723](https://peps.python.org/pep-0723/) style comment block embeds the conda
environment a script needs directly in the script file, using the same format as
[conda-exec](https://conda-incubator.github.io/conda-exec/). This makes scripts
self-contained and easy to share: anyone with `pixi` can run them with a single
command, without setting up a project first.

```python
#!/usr/bin/env python
# /// script
# requires-python = ">=3.12"
#
# [tool.conda]
# channels = ["conda-forge"]
# dependencies = ["your-package"]
# ///

import your_package
```

Run it with:

```bash
pixi exec your_script.py
```

The first run creates a cached environment and installs the dependencies;
subsequent runs reuse the cached environment.

## Examples

| Example                | Demonstrates                                                              |
| ---------------------- | ------------------------------------------------------------------------- |
| `hello_python.py`      | The minimal form: only `requires-python`, defaults for everything else    |
| `web_request.py`       | Conda dependencies (`requests`) under `[tool.conda]`                      |
| `platform_specific.py` | Platform-specific dependencies via `[tool.pixi.target.<selector>]`        |
| `hello_bash.sh`        | Non-Python scripts with an explicit `entrypoint` under `[tool.pixi]`      |
| `hello_node.js`        | A different comment syntax (`//`) with conda-managed Node.js              |

Run any of them with, for example:

```bash
pixi exec examples/script-metadata/hello_python.py
```

## Locking

To make a script reproducible, write a sidecar lock file next to it and share both
files:

```bash
pixi exec --lock examples/script-metadata/web_request.py
```

This writes `web_request.py.pixi.lock`; subsequent runs install from the lock file
instead of solving, as long as the metadata has not changed.

## Learn more

See the [documentation](https://pixi.sh/latest/advanced/script_metadata/) for the
complete syntax, platform selectors, entrypoint configuration, and locking.
