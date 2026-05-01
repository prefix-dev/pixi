# The Conda Ecosystem

Now that you've [created a workspace](first_workspace.md) and seen
[Pixi's basic commands](getting_started.md), you may have noticed
terms like channels, subdirs, and platforms. Pixi is built on the
conda packaging ecosystem, and this page explains what those terms
mean and how they fit together.

## What makes conda different?

Language package managers (pip, npm, cargo) handle only their own
language. System package managers (apt, dnf, pacman) cover everything
but install into a single global set of packages — you can't have two
versions of the same library side by side. Nix solves the versioning
problem but has a steep learning curve and its own configuration
language.

Conda sits in between: it is **language-agnostic** like a system
package manager — a single dependency tree can mix C libraries,
compilers, CLI tools, and language runtimes — but it installs into
**isolated environments** like a language package manager.

A few other things set it apart:

- **Binary-only distribution.** Packages are always pre-compiled, so
  installs are fast and don't require a build toolchain on the user's
  machine.
- **OS independent.** Packages can support common operating systems like Linux,
  macOS, Windows, and more.
- **Good support for host hardware.** Packages can be tailored very specifically
  to handle available hardware.
- **Independent environments.** Each set of requested packages installs into a
  self-contained directory. Nothing is shared with system packages or
  other environments.
- **All versions available at once.** Channels keep every published
  version and build of a package. The solver picks the right
  combination — you can pin `numpy 1.24` in one project and use
  `numpy 2.1` in another without conflict or even have different environments
  with different versions of `numpy` within one project.

## Packages

A **conda package** is an archive
([`.conda`](https://conda.io/projects/conda/en/latest/user-guide/concepts/packages.html)
or legacy `.tar.bz2`) that contains pre-compiled files and metadata.
The metadata declares the package's name, version, dependencies, and
which platform it was built for. The solver reads this metadata to
decide what to install — packages themselves are never executed during
resolution.

### Subdirs: how conda targets platforms

Conda packages are compiled for a specific **platform** (OS + architecture
combination). In conda terminology, each platform is called a **subdir** —
short for subdirectory, because channels store packages in directories
named after the platform they target:

```
conda-forge/
├── linux-64/        # Linux on x86_64
├── linux-aarch64/   # Linux on ARM64 (e.g. Graviton, Raspberry Pi 5)
├── osx-64/          # macOS on Intel
├── osx-arm64/       # macOS on Apple Silicon
├── win-64/          # Windows on x86_64
├── win-arm64/       # Windows on ARM64
└── noarch/          # platform-independent packages
```

When the solver resolves dependencies, it looks at the packages in
the subdir that matches the target platform, plus `noarch`. A package
in `noarch/` works on every platform — these are typically pure-Python
packages or data-only packages with no compiled code.

In a Pixi workspace you declare which platforms you support:

=== "pixi.toml"

    ```toml
    [workspace]
    platforms = ["linux-64", "osx-arm64", "win-64"]
    ```

=== "pyproject.toml"

    ```toml
    [tool.pixi.workspace]
    platforms = ["linux-64", "osx-arm64", "win-64"]
    ```

Pixi solves dependencies for each listed platform and records the
result in the lock file, even for platforms you're not currently
running on. This is what makes cross-platform lock files possible.
See [Multi-Platform Configuration](workspace/multi_platform_configuration.md)
for platform-specific dependencies and activation scripts.

### Variants: multiple builds of the same package

A single version of a package can be built in multiple ways. For
example, `numpy 2.1.0` might have separate builds for Python 3.11
and Python 3.12, or `pytorch 2.4.0` might have both CPU and CUDA
builds. In the conda ecosystem, these different builds of the same
version are called **variants**.

Each variant has a unique **build string** that encodes what makes it
different — typically the Python version, a hash of the build
configuration, and a build number:

```
numpy-2.1.0-py311h43a39b2_0.conda
            ^^^^^ ^^^^^^^ ^
            │     │       └─ build number
            │     └───────── configuration hash
            └─────────────── Python version
```

The solver picks the right variant automatically based on what's
already in your environment. If you've resolved Python 3.12, it
selects the `py312` variant of numpy. You can also pin a specific
build using the [MatchSpec](concepts/package_specifications.md) syntax:

```shell
pixi add "pytorch [build='cuda*']"
```

When _building_ packages with pixi-build, you can define variants
to produce multiple builds from a single source. See
[Build Variants](build/variants.md) for a tutorial.

### Virtual packages: describing the host system

Some packages need features that aren't provided by other packages
but by the **host system** itself — a minimum Linux kernel version,
a specific glibc, or an NVIDIA GPU driver. Conda models these
system capabilities as **virtual packages**: special packages with
names starting with `__` that the solver treats like any other
dependency, but that are never downloaded or installed.

Common virtual packages:

| Virtual package | What it represents |
|-----------------|--------------------|
| `__linux`       | Linux kernel version |
| `__glibc`       | GNU C Library version |
| `__osx`         | macOS version |
| `__cuda`        | NVIDIA driver's CUDA capability |
| `__archspec`    | CPU microarchitecture (e.g. `x86_64_v3`) |

When a package depends on `__cuda >= 12`, the solver will only
select it if the system provides a `__cuda` virtual package with
version 12 or higher. This is how the ecosystem prevents installing
GPU-accelerated packages on machines without a compatible GPU driver.

Pixi detects your system's virtual packages automatically. You
can also declare them explicitly in your workspace — for example, to
tell the solver that your deployment target has CUDA 12:

<!-- no-pyproject -->
```toml title="pixi.toml"
[system-requirements]
cuda = "12"
```

See [System Requirements](workspace/system_requirements.md) for
defaults, per-feature overrides, and environment variable overrides.

## Environments and prefixes

You've already seen that Pixi creates an environment for your
workspace in [First Workspace](first_workspace.md). In conda
terminology, an environment is also called a **prefix** — the
directory path that acts as the root of the environment (the install
prefix). Tools like conda and mamba typically store prefixes under
`~/miniconda3/envs/`. Pixi keeps them inside the workspace directory
(`.pixi/envs/`), so each project has its own isolated environments
that are easy to find and clean up.

A workspace can define
[multiple environments](workspace/multi_environment.md) — for example,
a `default` environment for development, a `test` environment with
extra test dependencies, and a `docs` environment for building
documentation. See [Environments](workspace/environment.md) for
more details.

Pixi can also install tools
[globally](global_tools/introduction.md) — outside of any workspace.
Global tools like `git`, `ripgrep`, or `ipython` each get their own
isolated prefix, so they don't conflict with each other or with your
project environments.

## Channels: where packages live

A **channel** is a repository of conda packages, served over HTTP. When
you add a dependency, Pixi fetches it from one or more channels. The
most important ones:

- **[conda-forge](https://conda-forge.org/)** - the community-maintained
  default channel with over 30,000 packages. This is where most
  open-source software lives.
- **[bioconda](https://bioconda.github.io/)** - the community-maintained channel focused on biomedical research.
- **[robostack-<ros-distro>](https://robostack.github.io/)** - the community-maintained channels for using ROS packages in the conda ecosystem.
- **[prefix.dev/github-releases](https://prefix.dev/channels/github-releases)** - automatically
generated conda packages from GitHub releases, so you can install CLI tools that don't have a conda-forge recipe yet.

All of these channels can be discovered and browsed on **[prefix.dev](https://prefix.dev/)** , or [host your own private channels](https://prefix.dev/channels/).

You can mix channels in a single workspace. For example, the
[PyTorch tutorial](python/pytorch.md) shows you how to combine different channels. See
[Channel Logic](advanced/channel_logic.md) for details on how channel
priority works.

## Tools in the ecosystem

Several tools can manage conda packages:

| Tool | Description |
|------|-------------|
| **[conda](https://docs.conda.io/)** | The original package manager, written in Python. Part of the Anaconda distribution. |
| **[mamba](https://mamba.readthedocs.io/)** | A faster drop-in replacement for conda, with a C++ solver. |
| **Pixi** | A modern, Rust-based package manager with lockfiles, tasks, and multi-environment support. Not a drop-in replacement — it rethinks the workflow. |

If you're coming from conda or mamba, see
[Switching from Conda](switching_from/conda.md) for a command-by-command
comparison.

## Rattler and rattler-build

Pixi is powered by **[Rattler](https://github.com/conda/rattler)**, a
set of Rust libraries that implement the conda ecosystem from scratch:
dependency resolution, package installation, channel interaction, and
environment management. Rattler is not a CLI tool — it's the engine
that Pixi (and other tools) build on top of.

Where Rattler _consumes_ packages, two tools help you _create_ them:

- **[pixi-build](build/getting_started.md)** is the simplest way to
  package your code. It reads your project metadata (e.g.
  `pyproject.toml`) and produces a conda package with minimal
  configuration — ideal for most projects.
- **[rattler-build](https://rattler.build)** gives you fine-grained
  control over every aspect of the packaging process: custom build
  scripts, patching sources, cross-compilation, and more. Use it when
  you need full control over how your package is built.

Both integrate with Pixi as
[build backends](build/backends/pixi-build-rattler-build.md), so you
can build, test, and publish packages from within a Pixi workspace.

For a deeper comparison of the conda and PyPI ecosystems and how Pixi
bridges them, see [Conda & PyPI](concepts/conda_pypi.md).


