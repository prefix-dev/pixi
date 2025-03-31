# Examples

## Adding a Series of Tools at Once

Without specifying an environment, you can add multiple tools at once:
```shell
pixi global install pixi-pack rattler-build
```
This command generates the following entry in the manifest:
```toml
[envs.pixi-pack]
channels = ["conda-forge"]
dependencies= { pixi-pack = "*" }
exposed = { pixi-pack = "pixi-pack" }

[envs.rattler-build]
channels = ["conda-forge"]
dependencies = { rattler-build = "*" }
exposed = { rattler-build = "rattler-build" }
```
Creating two separate non-interfering environments, while exposing only the minimum required binaries.

## Creating a Data Science Sandbox Environment

You can create an environment with multiple tools using the following command:
```shell
pixi global install --environment data-science --expose jupyter --expose ipython jupyter numpy pandas matplotlib ipython
```
This command generates the following entry in the manifest:
```toml
[envs.data-science]
channels = ["conda-forge"]
dependencies = { jupyter = "*", ipython = "*" }
exposed = { jupyter = "jupyter", ipython = "ipython" }
```
In this setup, both `jupyter` and `ipython` are exposed from the `data-science` environment, allowing you to run:
```shell
> ipython
# Or
> jupyter lab
```
These commands will be available globally, making it easy to access your preferred tools without switching environments.

## Install Packages For a Different Platform

You can install packages for a different platform using the `--platform` flag.
This is useful when you want to install packages for a different platform, such as `osx-64` packages on `osx-arm64`.
For example, running this on `osx-arm64`:
```shell
pixi global install --platform osx-64 python
```
will create the following entry in the manifest:
```toml
[envs.python]
channels = ["conda-forge"]
platforms = ["osx-64"]
dependencies = { python = "*" }
# ...
```
