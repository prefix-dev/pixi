A clear guide on how to install and setup your tool. 
Include instructions for different platforms (Windows, MacOS, Linux) if applicable. 
If the tool requires an understanding of certain concepts, provide explanations or links to further resources.

# Extended installation tutorial



# Understanding Pax's Directory Structure
The central philosophy of pax revolves around maintaining a separate conda environment for each project. 
The `pax` tool achieves this by organizing environments and installations within unique directories.

When you use pax to install a standalone tool, for example `cowpy`, it creates a distinct structure in your home directory. 
To illustrate, running the command `pax install cowpy` would yield the following directory structure:
```shell
$HOME
└── .pax
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
In this case, a cowpy directory is created within the `.pax/envs` directory, hosting the conda environment specific to `cowpy`.

Now, if you're working with `pax` on a project-level, the setup process is slightly different. 
You start by initializing an empty project with `pax` and then add the necessary tools or languages - in this case, `python`.

To visualize this, running the following set of commands:
```shell
pax init my_project && cd my_project
pax add python
```
...results in a unique project structure:
```shell

my_project
├── pax.toml
├── pax.lock
└── .pax
    └── env
        ├── bin
        │   ├── python
        │   └── ...
        ├── conda-meta
        ├── include
        ├── lib
        ├── ...
```
Here, a `.pax` directory is created within the `my_project` directory, containing the conda environment specific to `my_project`. 
The `pax.toml` and `pax.lock` files are also added to manage and lock the project's dependencies respectively.

# Basics of the configuration file
All Pax projects use a configuration file to setup the environment. 
For this the decision is made to use toml, as it is a properly supported format in Cargo(Rust's package manager) and `pyproject.toml`(multiple package managers).

Minimal example, that gets created by `pax init`
```shell
[project]
name = "my_project"                           # This is defaulted to the directory name or the name given to pax init
version = "0.1.0" 
description = "Add a short description here"
authors = ["John Doe <john@prefix.dev>"]      # Gets the information from you currently configured git user.
channels = ["conda-forge"]                    # Defaults to conda-forge, add in vector style more channels if needed.
platforms = ["linux-64"]                      # Defaults to the platform you are currently on but add the platform you want to support as needed.

[commands]
custom_command = "echo hello_world"

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
Currently the `name`, `version`, `description` and `authors` are just there for extra information.
As soon as we start supporting building packages this will directly be used.

### Name
The `name` gets automatically derived from the name you use in the `pax init NAME` command. 
Using `pax init .` will derive the name from the current folder.
### Authors
The `authors` get automatically derived from the git user. 
### Channels
As pax utilizes conda packages, it also supports the use of channels - a concept that allows packages to be fetched from multiple sources. 
For a detailed understanding, we recommend reading our [blog](https://prefix.dev/blog/introducing_channels) or consulting our [documentation](https://prefix.dev/docs/mamba/channels).

In brief, channels are defined within the `channels` section of your configuration. 
For example, if you need packages from `bioconda`, your configuration would look like this: 
```toml
channels = ["conda-forge", "bioconda"]
```
However, be mindful to explicitly specify all necessary channels. 
For instance, while `bioconda` relies on packages from `conda-forge`, there's no automatic dependency resolution for channels as of now. 
Hence, both `conda-forge` and `bioconda` need to be defined explicitly.

By clearly defining the necessary channels, you can ensure pax fetches the appropriate packages to fulfill your project's requirements.

### Platforms
To ensure that your project supports multiple platforms, you can specify these within the platforms key in your pax configuration.
**At least one** platform must be designated so that pax can determine the appropriate packages and record these in the lockfile. By doing so, pax will resolve the environment for each specified platform, ensuring multi-platform compatibility.

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
In addition to managing dependencies, `pax` aims to provide a user-friendly interface that simplifies the execution of repetitive, complex commands. 
The commands section in your `pax` configuration serves this purpose. 
Here, you can specify any commands that you frequently use in your project's environment.

Here are a few examples:
```toml
[commands]
build = "cargo build --release"
test = "pytest /tests"
check = "ruff check path/to/code/*.py"
```
With these commands specified in the configuration, you can easily execute them using `pax run`:
```shell
pax run build
pax run test
pax run check
```
This commands feature makes it straightforward and efficient to execute commonly-used commands, further enhancing `pax` as a versatile tool for project management.

## The `dependencies` part
As pax is a package manager we obviously provide a way to specify dependencies.
Dependencies are specified using a "version" or "version range" which for Conda is a "MatchSpec"

This is a conda specific way to specify dependencies, to avoid failing to write a good explination I'll link you to some excelent reads:
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
```

**Gotcha**: `python = "3"` resolves to `3.0.0` which is not a possible version. 
To get the lastest version of something always append with `.*` so that would be `python = "3.*"`
