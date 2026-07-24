# Running Python scripts

Pixi can run a local Python script in an isolated environment described by
metadata inside the file. Script commands use an explicit namespace:

```console
$ pixi script init example.py
$ pixi script add example.py rich
$ pixi script add --pypi example.py "requests>=2"
$ pixi script run example.py
```

There is no implicit `pixi script example.py` form. Writing `run` explicitly
keeps script paths distinct from subcommands and leaves room for more script
operations.

## Inline metadata

Pixi reads [PEP 723 inline script
metadata](https://packaging.python.org/en/latest/specifications/inline-script-metadata/).
It also supports the portable [`tool.conda` script
metadata](https://github.com/conda-incubator/conda-exec/blob/main/docs/reference/script-metadata.md)
used by conda-exec.

Initialize a new script, or add metadata to an existing one, with:

```console
$ pixi script init example.py
```

Pixi preserves an existing shebang and Python body. The generated metadata uses
the configured default channels and does not declare a platform:

```python title="example.py"
# /// script
# requires-python = ">=3.11"
# dependencies = []
#
# [tool.conda]
# channels = ["conda-forge"]
# dependencies = []
# ///

print("Hello world")
```

The fields have distinct roles:

- `requires-python` constrains the Python interpreter.
- The root `dependencies` array contains portable PEP 508 PyPI requirements.
- `tool.conda.channels` and `tool.conda.dependencies` contain portable conda
  channels and MatchSpecs.
- `tool.pixi` contains configuration that needs Pixi's richer manifest model.

If both `requires-python` and a conda `python` MatchSpec are present, Pixi
applies both constraints. Declaring the same normalized dependency in both its
portable and rich Pixi location is an error. Defining channels in both
`tool.conda` and `tool.pixi.workspace` is also an error.

## Adding and removing dependencies

Conda is the default ecosystem for `add`. Pixi writes the resolved MatchSpec to
the portable `tool.conda.dependencies` array:

```console
$ pixi script add example.py "rich>=14,<15"
```

Use `--pypi` explicitly for a PyPI requirement. Pixi writes it to the root PEP
723 dependency array:

```console
$ pixi script add --pypi example.py "requests>=2"
```

`add` initializes an existing script that does not yet contain a metadata
block, but only `pixi script init` creates a new file. `add` resolves and
installs the edited environment, but does not create a lock file unless a
sidecar lock already exists.

Removal infers the ecosystem from the existing declaration, so it does not
have a `--pypi` flag:

```console
$ pixi script remove example.py rich
$ pixi script remove example.py requests
```

If a name exists as both a conda and PyPI dependency, removal reports the
ambiguity. Conda and PyPI names cannot be mixed in one removal invocation.

Simple entries in `tool.pixi.dependencies`, `tool.pixi.pypi-dependencies`, or
`tool.pixi.workspace.channels` continue to work. Mutating commands warn when
such an entry could be moved to its portable location; Pixi does not rewrite it
automatically.

## Running a script

Run the script explicitly:

```console
$ pixi script run example.py
Hello world
```

Pixi options precede the script path. Everything after the path is forwarded to
Python:

```console
$ pixi script run --frozen example.py first --second
```

The script environment is independent of an enclosing Pixi workspace and an
active Pixi environment. Pixi stores it in the execution cache. Relative paths
in metadata are resolved from the script's directory, while the Python process
runs in the directory where the command was invoked.

The cache identity includes the script's stable absolute path. Pixi does not
canonicalize the final path component, so accessing a script through a symlink
uses the symlink path's environment, matching Pixi workspace and uv behavior.

## Locking dependencies

Create a Pixi sidecar lock explicitly:

```console
$ pixi script lock example.py
```

This writes `example.py.pixi.lock` next to the script; the lock is never
embedded. The initial lock targets the current platform. Specify an exact
platform set by repeating `--platform`:

```console
$ pixi script lock --platform linux-64 --platform osx-arm64 example.py
```

Once a sidecar exists, its platforms are reused by later `run`, `lock`, `add`,
and `remove` operations unless the script declares
`tool.pixi.workspace.platforms` or a `script lock --platform` invocation
replaces them.

Without a sidecar, `run`, `add`, and `remove` resolve without creating one.
With a sidecar, those commands maintain it. `--locked` requires an existing,
up-to-date lock, while `--frozen` requires an existing lock and trusts it
without checking metadata freshness.

The sidecar currently uses Pixi's `rattler-lock` format. Pixi does not yet read
or write conda-exec's `conda-exec.lock`; support for that format can be added
separately without changing inline metadata interoperability.

## Supported Pixi configuration

A script represents one implicit default environment. Pixi accepts this
explicit subset of `tool.pixi`:

- At `tool.pixi`: `activation`, `constraints`, `dependencies`,
  `pypi-dependencies`, `system-requirements`, `target`, and `workspace`.
- At `tool.pixi.workspace`: `channels`, `platforms`, `channel-priority`,
  `solve-strategy`, `requires-pixi`, `preview`, and `pypi-options`.
- In a target selector: `activation`, `constraints`, `dependencies`, and
  `pypi-dependencies`.

Unknown fields inside `tool.pixi` are rejected. Other tools' tables, such as
`tool.uv`, are ignored and preserved when Pixi edits the metadata. Tasks, named
features, environments, solve groups, and package or build configuration are
not available to a standalone script.

Only local script paths are supported. Standard input and remote script URLs
are not supported.
