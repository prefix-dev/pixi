# Standalone scripts in any language

Pixi can give a single code file its own environment, whatever the language.
A `conda-script` comment block inside the file declares the channels, the
dependencies and the command that runs it, so a C file, an R script or a
Python program travels as one self-contained file.

!!! warning "Experimental"
    This implements the draft `conda-script` proposal from
    [issue #3751](https://github.com/prefix-dev/pixi/issues/3751), which is
    on its way to becoming a CEP for the whole conda ecosystem. Running such
    a file therefore requires the `--experimental` flag, and details may
    still change. Feedback on the issue is very welcome.

```c title="main.c"
// /// conda-script
// channels = ["conda-forge"]
// entrypoint = "gcc -o ${CACHE}/main ${SCRIPT} $(pkg-config --cflags --libs glib-2.0) && ${CACHE}/main"
//
// [dependencies]
// gcc = "*"
// glib = "*"
// pkg-config = "*"
// /// end-conda-script
#include <glib.h>

int main(void) {
    gchar *digest = g_compute_checksum_for_string(G_CHECKSUM_SHA256, "conda-script", -1);
    g_print("sha256(\"conda-script\") = %s\n", digest);
    g_free(digest);
    return 0;
}
```

```shell
$ pixi run --experimental --script main.c
sha256("conda-script") = a733a69b1424e6d2f409c14dfb01c1c1558e0eb943786ea50eb04f55afe2226d
```

Pixi solves the dependencies, installs the environment into its cache and
runs the entrypoint inside it. Nothing is created next to the file.

For Python files there is also the standardized
[PEP 723 block](../python/scripts.md); a file may use either kind of block,
but not both.

## The block

A block is recognized without knowing the language's comment syntax:

- The opening line is any line ending in `/// conda-script`. Everything
  before the `///` is the *prefix*: the comment characters of the language.
  The prefix must be non-empty and free of letters and digits.
- Every following line starts with that exact prefix, and the block closes
  with the prefix followed by `/// end-conda-script`.
- Only line comments work, and a file contains at most one block.

The content is TOML, extended with multiline inline tables from the upcoming
TOML 1.1.

## Dependencies

`[dependencies]` maps conda package names to matchspecs. The string form is
a version:

```toml
[dependencies]
python = "3.13.*"
gcc = "*"
```

The table form supports `version`, `build`, `build-number`, `channel`,
`subdir`, `extras`, `flags`, `md5`, `sha256`, `url` and `when`. Platform
specific dependencies use
[conditional dependencies](../concepts/package_specifications.md#conditional-dependencies)
with virtual packages:

```toml
[dependencies]
gcc = { version = "*", when = "__unix" }
vs2022_win-64 = { version = "*", when = "__win" }
```

## The entrypoint

`entrypoint` is the command that runs the script. It is either a string or a
table keyed by platform, where the most specific key wins:

```toml
entrypoint = {
  unix = "cc -o ${CACHE}/main ${SCRIPT} && ${CACHE}/main",
  win = "cl /Fe:${CACHE}/main.exe ${SCRIPT} && ${CACHE}/main.exe",
}
```

The command is not passed to a system shell. Pixi runs it with a minimal,
fully specified syntax: whitespace splitting, single and double quotes,
`${VAR}` substitution, `$(command)` command substitution and `&&`
sequencing. There are no pipes, redirects, globbing, `||`, `;`, subshells or
environment variable assignments, so an entrypoint behaves the same on every
platform.

Two variables are defined:

- `${SCRIPT}`: the absolute path of the script file.
- `${CACHE}`: a persistent per-script directory for build artifacts and
  other state that survives between runs.

Arguments after the script path are appended to the last command:

```shell
$ pixi run --experimental --script main.c input.txt --verbose
# runs: gcc [...] && ${CACHE}/main input.txt --verbose
```

An argument list that starts with a flag needs `--` in front, so pixi does
not read the flag itself: `pixi run --experimental --script main.c -- --verbose`.

The entrypoint runs in the directory `pixi run` was invoked from, so
relative paths passed to the script work.

## Pixi-specific configuration

Tables under `[tool.*]` belong to the named tool. Pixi reads `[tool.pixi]`
the same way as in a `pyproject.toml`, restricted to one implicit
environment: `[tool.pixi.dependencies]` for specs in pixi's native syntax,
including [source dependencies](../build/dependency_types.md), and
`[tool.pixi.pypi-dependencies]` for PyPI packages.

`[tool.pixi.dependencies]` merges with `[dependencies]` the way pixi merges
features: every spec applies. A source dependency there composes with a
version constraint in `[dependencies]`, so tools that only implement the
conda-script specification still see a solvable script:

```toml
[dependencies]
simple-app = "0.1.*"

[tool.pixi.dependencies]
simple-app = { git = "https://github.com/prefix-dev/pixi-build-testsuite.git", subdirectory = "tests/data/pixi_build/minimal-backend-workspaces/pixi-build-python" }
```

## Locking

Running a script does not create a lock file; the resolution is cached
internally. To pin the environment, write a lock file next to the script:

```shell
pixi lock --script main.c
```

This creates `main.c.pixi.lock`, and a run uses the adjacent lock file
whenever it exists, the same convention as for
[PEP 723 scripts](../python/scripts.md#lock-exact-versions).

## Shebang

Since the shebang line lies outside the block, a Unix script can make itself
executable with [`env -S`](../advanced/shebang.md):

```sh
#!/usr/bin/env -S pixi run --experimental --script
```

The flag is inert for PEP 723 files, so the same shebang works for both
block kinds.
