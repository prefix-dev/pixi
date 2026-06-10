Sharing a script usually means sharing its environment as well: a `pixi.toml` or
`environment.yml` that describes the dependencies the script needs.
With an inline metadata block, that information lives inside the script file
itself, so a single file is all you need to share.
[`pixi exec`](../reference/cli/pixi/exec.md) reads the block, creates a cached
temporary environment, and runs the script in it:

```python title="fetch.py"
#!/usr/bin/env python
# /// script
# requires-python = ">=3.12"
#
# [tool.conda]
# channels = ["conda-forge"]
# dependencies = ["requests"]
# ///

import requests
print(requests.get("https://pixi.sh").status_code)
```

```shell
pixi exec fetch.py
```

The first run solves and installs the environment, subsequent runs reuse the cached
environment. Use `pixi clean cache --exec` to remove the cached environments.

The format follows [PEP 723](https://peps.python.org/pep-0723/) and is shared with
[conda-exec](https://conda-incubator.github.io/conda-exec/): conda dependencies and
channels live in the `[tool.conda]` table, while pixi-specific configuration lives
in `[tool.pixi]`.

## The metadata block

The metadata is a TOML document embedded in a comment block:

- The block starts with a line containing only `/// script` and ends with a line
  containing only `///`, each preceded by the comment marker.
- Every line in between must start with the same line-comment marker as the opening
  line: `#` (Python, shell, R, ...), `//` (JavaScript, Rust, ...), or `--`
  (SQL, Lua, Haskell, ...). Block comments such as `/* ... */` are not supported.
- The comment marker and at most one following space are stripped from each line;
  the remaining text is parsed as TOML.
- Following PEP 723, the block ends at the *last* `///` line of the comment block,
  and only the first metadata block in a file is read.

For example, the same metadata in a Node.js script:

```javascript title="hello.js"
// /// script
// [tool.conda]
// dependencies = ["nodejs 22.*"]
//
// [tool.pixi]
// entrypoint = "node"
// ///

console.log("Hello from Node.js!");
```

## Top-level keys

The top level of the document holds the standard PEP 723 keys:

- `requires-python`: a Python version specifier, e.g. `">=3.12"`. Pixi translates
  it into a conda `python` dependency.
- `dependencies`: PyPI requirements. These are **not supported yet** by `pixi exec`
  and produce an error; declare conda packages under `[tool.conda]` instead.

## `[tool.conda]`

The same table conda-exec reads: where the packages come from and which conda
packages the script needs. `dependencies` is a list of
[match specs](../reference/pixi_manifest.md#the-dependencies-tables):

```toml
[tool.conda]
channels = ["conda-forge", "bioconda"]
dependencies = ["python 3.12.*", "samtools>=1.19"]
```

When `channels` is omitted, the default channels are used (`conda-forge`, unless
[configured otherwise](../reference/pixi_configuration.md#default-channels)).
Channels passed via `--channel` on the command line take precedence. Similarly,
`--spec` on the command line replaces the dependencies from the script, while
`--with` adds packages alongside them.

## `[tool.pixi]`

Pixi-specific execution configuration. All of its keys are optional:

```toml
[tool.pixi]
entrypoint = "python"
```

- `entrypoint`: the command that runs the script. The script path is appended to
  it, so `entrypoint = "python"` runs `python <script>`. Use the `${SCRIPT}`
  placeholder to position the script path yourself, e.g.
  `entrypoint = "bash -e ${SCRIPT}"`. Any extra arguments after the script on the
  `pixi exec` command line are passed through.

  Without an `entrypoint`, a `.py` script that declares a Python requirement is run
  with `python`, like conda-exec does. Any other script is executed directly, which
  requires it to be executable (and on Unix-like systems to have a
  [shebang line](shebang.md)).

Platform-specific dependencies and entrypoints go into
`[tool.pixi.target.<selector>]` sub-tables, where the selector is `unix`, `linux`,
`osx`, `win`, or a concrete conda subdir such as `linux-64`:

```toml
[tool.conda]
dependencies = ["cmake"]

[tool.pixi]
entrypoint = "bash ${SCRIPT}"

[tool.pixi.target.linux]
dependencies = ["patchelf"]

[tool.pixi.target.win]
dependencies = ["vs2022_win-64"]
entrypoint = "cmd.exe /c ${SCRIPT}"
```

## Locking

To make a script reproducible across machines and over time, record the resolved
environment in a sidecar lock file:

```shell
pixi exec --lock fetch.py
```

This solves the environment (or reuses the cached one), runs the script, and writes
`fetch.py.pixi.lock` next to it. Commit both files to version control. As long as
the metadata block does not change, subsequent runs install from the lock file
instead of solving:

```shell
pixi exec fetch.py
```

The lock file records a digest of the metadata block it was created from. When the
metadata changes, the lock file is considered out of date: runs fall back to
solving (with a warning), and `pixi exec --lock` refreshes the lock file. Locking
on another platform adds that platform's resolution to the file without discarding
existing ones.

- Use `--ignore-lock` to solve from the metadata for a single run without reading
  or modifying the lock file.
- Use `--lock --force-reinstall` to force a fresh solve and refresh the lock file
  even when the metadata has not changed.

## Examples

The pixi repository contains
[runnable examples](https://github.com/prefix-dev/pixi/tree/main/examples/script-metadata)
covering Python, Bash, Node.js, and platform-specific dependencies.
