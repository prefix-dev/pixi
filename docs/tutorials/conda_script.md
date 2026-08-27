# Standalone scripts in any language

With conda scripts you can run a self contained script written in any language that includes dependencies and entry-point.

!!! warning "Experimental"
    This implements the draft `conda-script` proposal from [issue #3751](https://github.com/prefix-dev/pixi/issues/3751), which is on its way to becoming a CEP for the whole conda ecosystem.
    Opt in with `pixi config set experimental.conda-script true --global` for now, and let us know on the issue how it goes.

Let's take this script written in R for example.
This is the most optimal use case for `conda-script`, R is meant for scripting, `conda-forge` features a wide selection of R libraries and [unlike Python](../python/scripts.md) it doesn't have its own script syntax.

```r title="main.R"
--8<-- "docs/source_files/conda_scripts/main.R"
```

Every conda script needs to declare the channels where the packages come from and the entrypoint describing how the script should be run.
Typically, you also want to add the toolchain of your language (`r-base`) and maybe a few dependencies (`r-jsonlite`) in order to make sure the script is self-contained.
In the script itself, we create a variable called `document` and then print it as JSON.

We can then run it with by executing the following command:

```shell
$ pixi run --script main.R
{
  "name": "conda-script",
  "languages": ["r", "python"],
  "count": 2
}
```

Pixi solves the dependencies, installs the environment into its cache and runs the entrypoint inside it.

## More languages

Ideally, dependencies also come from a conda channel.
Here are examples for languages that have a great selection of libraries on conda-forge or other channels.

=== "Python"

    ```py title="main.py"
    --8<-- "docs/source_files/conda_scripts/main.py"
    ```

=== "R"

    ```r title="main.R"
    --8<-- "docs/source_files/conda_scripts/main.R"
    ```

=== "C"

    ```c title="main.c"
    --8<-- "docs/source_files/conda_scripts/main.c"
    ```

=== "C++"

    ```cpp title="main.cpp"
    --8<-- "docs/source_files/conda_scripts/main.cpp"
    ```

=== "Fortran"

    ```fortran title="main.f90"
    --8<-- "docs/source_files/conda_scripts/main.f90"
    ```

=== "Mojo"

    ```mojo title="main.mojo"
    --8<-- "docs/source_files/conda_scripts/main.mojo"
    ```

Other languages work well with conda script, even though only the toolchain is available.
That is because they allow to specify dependencies as part of the program.

=== "C#"

    ```csharp title="main.cs"
    --8<-- "docs/source_files/conda_scripts/main.cs"
    ```

=== "Kotlin"

    ```kotlin title="main.main.kts"
    --8<-- "docs/source_files/conda_scripts/main.main.kts"
    ```

=== "TypeScript"

    ```typescript title="main.ts"
    --8<-- "docs/source_files/conda_scripts/main.ts"
    ```

## Creating a script

`pixi init --script <PATH>` writes a runnable starting point, choosing the comment syntax, an entrypoint and the toolchain dependency from the file extension:

```shell
$ pixi init --script main.R
$ pixi run --script main.R
Hello from pixi!
```

A Python file gets a [PEP 723 block](../python/scripts.md) instead; `--format conda-script` overrides that default.
Unknown extensions error and list the supported ones.

## The comment block

The metadata lives in a comment block at the top of the file, written with your language's own line comments.
The file therefore stays valid source code that editors, formatters and the language's own tooling keep understanding.

Open the block with `/// conda-script`, start every line with the same comment characters and close it with `/// end-conda-script`:

```r
# /// conda-script
# channels = ["https://prefix.dev/conda-forge"]
# entrypoint = "Rscript ${SCRIPT}"
# /// end-conda-script
```

C uses `//` for the same block, Fortran `!`.
Any comment characters work as long as they contain no letters or digits, which rules out languages that spell their comments as a word like `REM`.
Block comments such as `/* */` are not supported, and a file holds at most one block.

Inside the block you write TOML 1.1, so inline tables may span several lines.

## Dependencies

`[dependencies]` maps conda package names to matchspecs.
The string form is a version:

```toml
[dependencies]
python = "3.13.*"
gcc = "*"
```

The table form supports `version`, `build`, `build-number`, `channel`, `subdir`, `extras`, `flags`, `md5`, `sha256`, `url` and `when`.
Platform specific dependencies use [conditional dependencies](../concepts/package_specifications.md#conditional-dependencies) with virtual packages:

```toml
[dependencies]
gcc = { version = "*", when = "__unix" }
vs2022_win-64 = { version = "*", when = "__win" }
```

## The entrypoint

`entrypoint` is the command that runs the script.
It is either a string or a table keyed by platform, where the most specific key wins:

```toml
entrypoint = {
  unix = "cc -o ${CACHE}/main ${SCRIPT} && ${CACHE}/main",
  win = "cl /Fe:${CACHE}/main.exe ${SCRIPT} && ${CACHE}/main.exe",
}
```

The command is not passed to a system shell.
Instead, Pixi runs a built-in shell so it behaves the same on every platform.
The following syntax is supported: whitespace splitting, single and double quotes, `${VAR}` substitution, `$(command)` command substitution and `&&` sequencing.
There are no pipes, redirects, globbing, `||`, `;`, subshells or environment variable assignments.

Two variables are defined:

- `${SCRIPT}`: the absolute path of the script file.
- `${CACHE}`: a persistent per-script directory for build artifacts and other state that survives between runs.

Arguments after the script path are appended to the last command:

```shell
$ pixi run --script main.R input.txt --verbose
# runs: Rscript ${SCRIPT} input.txt --verbose
```

An argument list that starts with a flag needs `--` in front, so pixi does not read the flag itself: `pixi run --script main.R -- --verbose`.

The entrypoint runs in the directory `pixi run` was invoked from, so relative paths passed to the script work.

## Pixi-specific configuration

Tables under `[tool.*]` belong to the named tool. Pixi reads `[tool.pixi]`
the same way as in a `pyproject.toml`, restricted to one implicit
environment: `[tool.pixi.dependencies]` for specs in pixi's native syntax,
including [source dependencies](../build/dependency_types.md), and
`[tool.pixi.pypi-dependencies]` for PyPI packages.

`[tool.pixi.dependencies]` merges with `[dependencies]` the way pixi merges features: every spec applies.
A source dependency there composes with a version constraint in `[dependencies]`, so tools that only implement the conda-script specification still see a solvable script:

```toml
[dependencies]
simple-app = "0.1.*"

[tool.pixi.dependencies]
simple-app = { git = "https://github.com/prefix-dev/pixi-build-testsuite.git", subdirectory = "tests/data/pixi_build/minimal-backend-workspaces/pixi-build-python" }
```

## Managing dependencies

The `--script` commands work on conda-script files the same way they work on PEP 723 scripts.
`pixi add` edits the block in place, preserving the comment prefix and the code around it:

```shell
$ pixi add --script main.c zlib      # writes [dependencies]
$ pixi add --script main.py --pypi rich  # writes [tool.pixi.pypi-dependencies]
```

`pixi list --script`, `pixi tree --script` and `pixi update --script` read and refresh the same environment.
Since the block cannot express platform-specific tables or git specs under `[dependencies]`, `pixi add` rejects `--platform` and `--git` with a hint towards `when` conditions and `[tool.pixi.dependencies]`.

## Locking

Running a script does not create a lock file; the resolution is cached internally.
To pin the environment, write a lock file next to the script:

```shell
pixi lock --script main.c
```

This creates `main.c.pixi.lock`, and a run uses the adjacent lock file whenever it exists, the same convention as for [PEP 723 scripts](../python/scripts.md#lock-exact-versions).

## Shebang

Since the shebang line lies outside the block, a Unix script can make itself executable with [`env -S`](../advanced/shebang.md):

```sh
#!/usr/bin/env -S pixi run --script
```

The same shebang works for a [PEP 723 script](../python/scripts.md), so both block kinds share it.
