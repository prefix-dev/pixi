This guide helps you transition from uv to Pixi.
It compares commands and concepts between the two tools, and explains what Pixi adds on top: the full conda ecosystem for managing non-Python dependencies, system libraries, and multi-language projects.

## Why Pixi?

uv is a fast Python package manager, but it's limited to the PyPI ecosystem. Pixi builds on conda, which brings several fundamental advantages:

- **System dependencies included.** Need CUDA, OpenSSL, compilers, or C libraries? Conda packages bundle them. With uv, you have to install these yourself via `apt`, `brew`, Docker, or manual setup.
- **Multi-language support.** A single Pixi workspace can manage Python, R, C/C++, Rust, Node.js, and more, while uv only handles Python.
- **Binary-first distribution.** Conda packages are pre-compiled, so you rarely need a build toolchain on your machine. No waiting for source builds or debugging missing C headers.
- **Complete environment modeling.** Conda environments contain everything (interpreters, libraries, headers, compilers, CLI tools), all managed by the solver. With uv, your Python environment depends on whatever your system happens to provide.
- **True cross-platform lockfiles.** Pixi solves for all target platforms in a single lockfile, even platforms you're not currently running on.
- **Built-in task runner.** Define and run tasks directly in your manifest, no need for `Makefile`, `just`, or shell scripts.

!!! tip "You can still use PyPI packages"
    Pixi fully supports PyPI packages alongside conda packages, powered by uv under the hood.
    Use `pixi add --pypi <package>` to add PyPI dependencies, or define them in `[project.dependencies]` when using `pyproject.toml`.
    See [Conda & PyPI](../concepts/conda_pypi.md) for how the two ecosystems work together.

## Quick look at the differences

| Task                      | uv                                | Pixi                                                                                      |
|---------------------------|-----------------------------------|-------------------------------------------------------------------------------------------|
| Creating a project        | `uv init myproject`               | `pixi init myproject`                                                                     |
| Adding a dependency       | `uv add numpy`                    | `pixi add numpy` (conda) or `pixi add --pypi numpy` (PyPI)                               |
| Removing a dependency     | `uv remove numpy`                 | `pixi remove numpy` (conda) or `pixi remove --pypi numpy` (PyPI)                         |
| Installing/syncing        | `uv sync`                         | `pixi install`                                                                            |
| Running a command         | `uv run python main.py`           | `pixi run python main.py`                                                                 |
| Running a standalone script | `uv run script.py` (PEP 723)   | `pixi exec` via [shebang](../advanced/shebang.md)                                        |
| Running a task            | _(no built-in task runner)_       | `pixi run my_task`                                                                        |
| Locking dependencies      | `uv lock`                         | `pixi lock` (also runs automatically on `pixi add` / `pixi install`)                     |
| Installing Python         | `uv python install 3.12`          | `pixi add python=3.12` (managed as a regular dependency)                                  |
| Ephemeral tool execution  | `uvx ruff check`                  | `pixi exec ruff check`                                                                    |
| Global tool install       | `uv tool install ruff`            | `pixi global install ruff`                                                                |
| Building a package        | `uv build`                        | Supported via [pixi-build backends](../build/getting_started.md)                          |
| Publishing a package      | `uv publish`                      | Upload to a [prefix.dev channel](../deployment/prefix.md)                                 |
| Exporting a lockfile      | `uv export`                       | `pixi workspace export conda-environment`                                                 |
| Virtual environments      | `.venv/` (automatic)              | `.pixi/envs/` (automatic, supports multiple environments)                                 |
| Cache management          | `uv cache clean`                  | `pixi clean cache`                                                                        |
| Updating dependencies     | `uv lock --upgrade`               | `pixi update`                                                                             |
| GitHub Actions            | `astral-sh/setup-uv`             | `prefix-dev/setup-pixi`                                                                   |

## Project configuration

uv uses `pyproject.toml` for project configuration and `uv.toml` for tool-level settings. Pixi supports both `pixi.toml` (its native format) and `pyproject.toml` for project configuration, and uses a separate [configuration file](../reference/pixi_configuration.md) for tool-level settings.

=== "uv (pyproject.toml)"

    ```toml
    [project]
    name = "myproject"
    version = "0.1.0"
    requires-python = ">=3.12"
    dependencies = [
        "numpy>=1.26",
        "pandas>=2.0",
    ]

    [dependency-groups]
    dev = ["pytest>=8.0"]
    ```

=== "Pixi (pixi.toml)"

    ```toml
    [workspace]
    name = "myproject"
    channels = ["conda-forge"]
    platforms = ["linux-64", "osx-arm64", "win-64"]

    [dependencies]
    python = ">=3.12"
    numpy = ">=1.26"
    pandas = ">=2.0"

    [feature.test.dependencies]
    pytest = ">=8.0"

    [environments]
    test = ["test"]
    ```

=== "Pixi (pyproject.toml)"

    ```toml
    [project]
    name = "myproject"
    version = "0.1.0"
    requires-python = ">=3.12"
    dependencies = [
        "numpy>=1.26",
        "pandas>=2.0",
    ]

    [dependency-groups]
    test = ["pytest>=8.0"]

    [tool.pixi.workspace]
    channels = ["conda-forge"]
    platforms = ["linux-64", "osx-arm64", "win-64"]

    [tool.pixi.environments]
    test = { features = ["test"], solve-group = "default" }
    ```

With `pyproject.toml`, Pixi reads `[project.dependencies]` as PyPI dependencies and `[tool.pixi.dependencies]` as conda dependencies. See the [pyproject.toml guide](../python/pyproject_toml.md) for details.

## Concepts mapping

### Python version management

uv manages Python installations separately with `uv python install`. In Pixi, Python is just another package:

```shell
pixi add python=3.12    # add Python as a conda dependency
```

Python gets version-locked in your lockfile alongside everything else, so there's no separate `.python-version` file to manage.

### Virtual environments

uv creates a single `.venv/` directory per project. Pixi creates environments under `.pixi/envs/`, and supports **multiple named environments** that exist simultaneously in one workspace:

```toml title="pixi.toml"
[environments]
default = []
test = ["test"]
docs = ["docs"]
cuda = ["cuda"]
```

Each environment can have completely different (even conflicting) dependencies, and Pixi keeps them all installed side by side. For example, you can have one environment with `numpy 1.x` and another with `numpy 2.x`, both ready to use without reinstalling anything.

uv can resolve conflicting dependency groups separately in the lockfile via `tool.uv.conflicts`, but it still uses a single `.venv/` that you swap between with `uv sync --group <name>`. Pixi environments are independent directories, so switching is instant.

See [Multi Environment](../workspace/multi_environment.md).

### Dependency groups and extras

uv uses [PEP 735 dependency groups](https://peps.python.org/pep-0735/) and optional dependencies (extras) to organize dependencies. Pixi uses **features**, composable sets of dependencies that map to environments:

| uv                           | Pixi                                                                |
|------------------------------|---------------------------------------------------------------------|
| `[dependency-groups]`        | `[feature.<name>.dependencies]`                                     |
| `[project.optional-dependencies]` | `[feature.<name>.dependencies]` mapped to environments          |
| `uv sync --group dev`       | `pixi install -e dev`                                               |
| `uv sync --all-groups`      | `pixi install --all`                                                |

Features are more flexible than dependency groups: they can include conda dependencies, platform-specific packages, system requirements, and activation scripts.

### Workspaces

Both tools support multi-package workspaces. uv defines workspace members with a glob pattern in `pyproject.toml`:

```toml title="uv pyproject.toml"
[tool.uv.workspace]
members = ["packages/*"]
```

Pixi takes a different approach: you reference local packages as path dependencies directly in the workspace manifest. Any subdirectory with its own `pixi.toml` (containing a `[package]` section) can be pulled in this way:

```toml title="pixi.toml"
[workspace]
channels = ["conda-forge"]
platforms = ["linux-64", "osx-arm64", "win-64"]

[dependencies]
my_lib = { path = "packages/my_lib" }
```

Both tools share a single lockfile across the workspace. See [Building Multiple Packages](../build/workspace.md).

### Standalone scripts

uv supports [PEP 723 inline script metadata](https://peps.python.org/pep-0723/) for standalone scripts that declare their own dependencies:

```python title="uv script"
# /// script
# requires-python = ">=3.12"
# dependencies = ["requests"]
# ///
import requests
print(requests.get("https://example.com").status_code)
```

Pixi has a similar capability via [shebang scripts](../advanced/shebang.md) using `pixi exec`, which creates a temporary environment with the specified dependencies:

```python title="pixi shebang script"
#!/usr/bin/env -S pixi exec --spec requests --spec python=3.12 -- python
import requests
print(requests.get("https://example.com").status_code)
```

This works on Linux and macOS. A more complete scripting feature is under discussion in [#3751](https://github.com/prefix-dev/pixi/issues/3751).

### Tasks

uv has no built-in task runner. Pixi does:

```toml title="pixi.toml"
[tasks]
start = "python main.py"
test = "pytest"
lint = "ruff check ."
check = { depends-on = ["lint", "test"] }  # task dependencies
fmt = { cmd = "ruff format .", env = { RUFF_LINE_LENGTH = "120" } }
```

```shell
pixi run check   # runs lint then test
pixi run start
```

Tasks support inter-task dependencies, environment variables, working directory configuration, and cross-platform commands. See [Tasks](../workspace/advanced_tasks.md).

### Ephemeral tool execution (`uvx` vs `pixi exec`)

`uvx` (short for `uv tool run`) runs a tool in a temporary environment without installing it permanently. `pixi exec` does the same thing:

| uv                              | Pixi                               |
|---------------------------------|------------------------------------|
| `uvx ruff check`               | `pixi exec ruff check`            |
| `uvx --from 'ruff>=0.5' ruff check` | `pixi exec --spec 'ruff>=0.5' ruff check` |
| `uvx --with numpy ruff check`  | `pixi exec --with numpy ruff check` |

### Global tools (`uv tool` vs `pixi global`)

Both tools install CLI tools globally in isolated environments:

| uv                              | Pixi                               |
|---------------------------------|------------------------------------|
| `uv tool install ruff`         | `pixi global install ruff`         |
| `uv tool list`                 | `pixi global list`                 |
| `uv tool uninstall ruff`       | `pixi global uninstall ruff`       |

Because Pixi global tools come from the conda ecosystem, you can install non-Python tools too:

```shell
pixi global install git bat ripgrep starship
```

See [Global Tools](../global_tools/introduction.md).

### Package indexes and channels

uv uses PyPI as its default package index, with support for custom indexes via `[[tool.uv.index]]`.

Pixi uses **conda channels**, repositories of pre-compiled packages. The default is [conda-forge](https://conda-forge.org/), the largest community-maintained channel:

```toml title="pixi.toml"
[workspace]
channels = ["conda-forge"]
# Add additional channels:
# channels = ["conda-forge", "pytorch", "https://my-company.com/channel"]
```

For private packages, you can host your own channel on [prefix.dev](https://prefix.dev/), S3, or JFrog Artifactory. See [Authentication](../deployment/authentication.md).

### Lockfiles

Both tools generate lockfiles for reproducibility.

| Aspect             | uv (`uv.lock`)         | Pixi (`pixi.lock`)                                   |
|--------------------|------------------------|-------------------------------------------------------|
| Format             | TOML                   | YAML                                                  |
| Cross-platform     | Universal resolution   | Solves per-platform, stored in one file                |
| Multi-environment  | Single resolution      | Per-environment resolution                            |
| Package types      | PyPI only              | Conda + PyPI                                          |
| Generate/update    | `uv lock`              | `pixi lock` (also automatic on `pixi add` / `pixi install`) |

See [Lock File](../workspace/lockfile.md).

### Building and publishing

uv builds Python packages with `uv build` (PEP 517 backends) and publishes to PyPI with `uv publish`.

Pixi builds packages via [pixi-build](../build/getting_started.md), which produces conda packages from Python, C++, Rust, ROS, and more. You can publish them to a [prefix.dev channel](../deployment/prefix.md) or any conda channel.

### CI with GitHub Actions

uv provides [`astral-sh/setup-uv`](https://github.com/astral-sh/setup-uv) for GitHub Actions. Pixi has [`prefix-dev/setup-pixi`](https://github.com/prefix-dev/setup-pixi), which installs Pixi, sets up caching, and runs `pixi install` in your workflow:

```yaml
- uses: prefix-dev/setup-pixi@v0.8.8
```

See [GitHub Actions](../integration/ci/github_actions.md) for more details.

### The `uv pip` interface

uv provides a `uv pip` compatibility layer (`uv pip install`, `uv pip compile`, etc.).

Pixi has no pip compatibility layer, it manages all dependencies declaratively through the manifest file. If you need pip for a specific use case, you can install it as a dependency:

```shell
pixi add pip
# not recommended, prefer pixi add --pypi
pixi run pip install <some-package>
```

!!! warning "Prefer `pixi add --pypi`"
    Using `pip` inside a Pixi environment bypasses the solver and lockfile.
    Always prefer `pixi add --pypi <package>` to keep dependencies tracked and reproducible.

## Why the conda ecosystem matters

If you're coming from uv, you might wonder why conda packages matter when PyPI already has everything you need.

### System dependencies are included

With uv, installing `scipy` or `pytorch` often requires system-level libraries (BLAS, LAPACK, CUDA) to already be on your machine. This leads to platform-specific setup instructions, Docker containers just for build deps, or cryptic build failures.

With Pixi, these system dependencies are conda packages, managed by the solver like any other dependency:

```shell
# CUDA runtime, cuDNN, and all system libraries are resolved automatically
pixi add pytorch-gpu
```

### Reproducibility beyond Python

`uv.lock` captures your Python dependencies precisely, but your project also depends on the system's C compiler, CUDA version, OpenSSL build, and more, none of which are tracked.

`pixi.lock` captures **everything**: the Python interpreter, system libraries, compilers, and CLI tools. When a colleague clones your project and runs `pixi install`, they get the exact same environment. No "works on my machine" surprises.

### No Docker needed for environment isolation

A common pattern with uv is using Docker to get a reproducible environment with the right system dependencies. Pixi environments achieve the same isolation without containers: no root privileges required, dramatically smaller than container images, instant creation, and the same reproducibility guarantees via the lockfile.

### Forward-compatible

Conda packages compile against the oldest supported system baseline, so they work on newer OS versions too. Your lockfile from today will still install correctly on next year's OS release.

For a deeper dive into the differences between the conda and PyPI ecosystems, see the [Conda != PyPI](https://conda.org/blog/conda-is-not-pypi) blog post series.

## Migrating a project

To migrate an existing uv project to Pixi, start by initializing Pixi in your project directory:

=== "pixi.toml"

    ```shell
    pixi init --format pixi
    ```

    This creates a `pixi.toml` alongside your `pyproject.toml`.

=== "pyproject.toml"

    ```shell
    pixi init --format pyproject
    ```

    This adds a `[tool.pixi.workspace]` section to your existing `pyproject.toml`, keeping your PyPI dependencies in place.

1. **Where possible, use conda-forge packages instead of PyPI:**

    Conda packages bundle system libraries and pre-compiled binaries, so `pixi add numpy` gives you numpy with BLAS, LAPACK, and everything else included. Use `pixi add --pypi <package>` only for packages that aren't available on conda-forge. If a package you need is missing from conda-forge, consider [adding it yourself](https://github.com/pavelzw/skill-forge/blob/main/recipes/conda-forge/SKILL.md).

2. **Set up tasks to replace your scripts:**

    ```shell
    pixi task add test "pytest"
    pixi task add lint "ruff check ."
    pixi task add serve "python -m http.server"
    ```

5. **Run your project:**

    ```shell
    pixi run test
    pixi run python main.py
    ```

Once everything works with `pixi.lock`, you can remove `uv.lock`.
