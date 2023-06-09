# Understanding Pixi's Directory Structure
The central philosophy of pixi revolves around maintaining a separate conda environment for each project.
The `pixi` tool achieves this by organizing environments and installations within unique directories.

When you use pixi to install a standalone tool, for example `cowpy`, it creates a distinct structure in your home directory.
To illustrate, running the command `pixi install cowpy` would yield the following directory structure:
```shell
$HOME
└── .pixi
    ├── bin
    │   └── cowpy
    └── envs
        └── cowpy
            ├── bin
            ├── conda-meta
            ├── include
            ├── lib
            ├── ...
```
In this case, a cowpy directory is created within the `.pixi/envs` directory, installing the conda environment specific to `cowpy`.

Now, if you're working with `pixi` on a project-level, the setup process is slightly different.
You start by initializing an empty project with `pixi` and then add the necessary tools, compilers, interpreters, and dependencies - in this case, `python`.

To visualize this, running the following set of commands:
```shell
pixi init my_project
cd my_project
pixi add python
```
...results in a unique project structure:
```shell

my_project
├── pixi.toml
├── pixi.lock
└── .pixi
    └── env
        ├── bin
        │   ├── python
        │   └── ...
        ├── conda-meta
        ├── include
        ├── lib
        ├── ...
```
Here, a `.pixi` directory is created within the `my_project` directory, containing the conda environment specific to `my_project`.
The `pixi.toml` and `pixi.lock` files are also added to manage and lock the project's dependencies respectively.

# Basics of the configuration file
All Pixi projects use a configuration file to setup the environment.
A project file is written in toml format similar to Cargo.toml (Rust) and `pyproject.toml`(Python).

Minimal example, that gets created by `pixi init`
```shell
[project]
name = "my_project"                           # This is defaulted to the directory name or the name given to pixi init
version = "0.1.0"
description = "Add a short description here"
authors = ["John Doe <john@prefix.dev>"]      # Gets the information from you currently configured git user.
channels = ["conda-forge"]                    # Defaults to conda-forge, add in vector style more channels if needed.
platforms = ["linux-64"]                      # Defaults to the platform you are currently on but add the platform you want to support as needed.

[commands]

[dependencies]
```
## The `project` part.
```shell
[project]
name = "my_project"
version = "0.1.0"
description = "Add a short description here"
authors = ["John Doe <john@prefix.dev>"]
channels = ["conda-forge"]
platforms = ["linux-64"]
```
Currently, the `name`, `version`, `description` and `authors` are just there for extra information.
As soon as we start supporting building packages this will directly be used.

### Name
The `name` gets automatically derived from the name you use in the `pixi init NAME` command.
Using `pixi init .` will derive the name from the current folder.
### Authors
The `authors` gets automatically added by finding your git user and email adress from the `git config`
### Channels
As pixi utilizes conda packages, it also supports the use of channels - a concept that allows packages to be fetched from multiple sources.
For a detailed understanding, we recommend reading our [blog](https://prefix.dev/blog/introducing_channels) or consulting our [documentation](https://prefix.dev/docs/mamba/channels).

In brief, channels are defined within the `channels` section of your configuration.
For example, if you need packages from `bioconda`, your configuration would look like this:
```toml
channels = ["conda-forge", "bioconda"]
```
However, be mindful to explicitly specify all necessary channels.
For instance, while `bioconda` relies on packages from `conda-forge`, there's no automatic dependency resolution for channels as of now.
Hence, both `conda-forge` and `bioconda` need to be defined explicitly.

By clearly defining the necessary channels, you can ensure pixi fetches the appropriate packages to fulfill your project's requirements.

### Platforms
To ensure that your project supports multiple platforms, you can specify these within the platforms key in your pixi configuration.
**At least one** platform must be designated so that pixi can determine the appropriate packages and record these in the lockfile. By doing so, pixi will resolve the environment for each specified platform, ensuring multi-platform compatibility.

The platforms that can be included are:

```toml
platforms = [
"linux-64",
"linux-aarch64",
"linux-ppc64le",
"osx-64",
"osx-arm64",
"win-64"
]
```
However, it's important to be aware that not every conda package supports all platforms.
To ascertain the platform support for each package, you can visit our website at [prefix.dev](https://prefix.dev).

For instance, to check the platform compatibility of the python package, you can follow this link: [python](https://prefix.dev/channels/conda-forge/packages/python)

Incorporating the appropriate platform configurations in your project ensures its broad usability and accessibility across various environments.

## The `commands` part
In addition to managing dependencies, `pixi` aims to provide a user-friendly interface that simplifies the execution of repetitive, complex commands.
The commands section in your `pixi` configuration serves this purpose.
Here, you can specify any commands that you frequently use in your project's environment.

Here are a few examples:
```toml
[commands]
build = "cargo build --release"
test = {cmd = "pytest /tests", depends_on=["build"]}

[commands.check]
cmd = "ruff check path/to/code/*.py"
```
With these commands specified in the configuration, you can easily execute them using `pixi run`:
```shell
pixi run build
pixi run test
pixi run check
```
This commands feature makes it straightforward and efficient to execute commonly-used commands, further enhancing `pixi` as a versatile tool for project management.

The `depends_on` will run the specified command in there to be run before the command itself.
So in the example `build` will be run before `test`.
`depends_on` can be a string or a list of strings e.g.: `depends_on="build"` or `depends_on=["build", "anything"]`

## The `dependencies` part
As pixi is a package manager we obviously provide a way to specify dependencies.
Dependencies are specified using a "version" or "version range" which for Conda is a "MatchSpec"

This is a conda specific way to specify dependencies, to avoid failing to write a good explanation I'll link you to some excellent reads:
- [Conda build documentation](https://docs.conda.io/projects/conda-build/en/latest/resources/package-spec.html#id6)
- [Excelent stackoverflow answer](https://stackoverflow.com/a/57734390/13258625)
- [Conda's python implementation](https://github.com/conda/conda/blob/main/conda/models/match_spec.py)
- [Rattler's rust implementation(ours)](https://github.com/mamba-org/rattler/blob/main/crates/rattler_conda_types/src/match_spec/mod.rs)

Here are some examples:
```toml
[dependencies]
python = "3.*"
python = "3.7.*"
python = "3.7.10.*"
python = "3.8.2 h8356626_7_cpython"
python = ">3.8.2"
python = "<3.8.2"
python = ">=3.8.2"
python = "<=3.8.2"
python = ">=3.8,<3.9"
python = "3.11"
python = "3.10.9|3.11.*"
```

**Gotcha**: `python = "3"` resolves to `3.0.0` which is not a possible version.
To get the latest version of something always append with `.*` so that would be `python = "3.*"`
