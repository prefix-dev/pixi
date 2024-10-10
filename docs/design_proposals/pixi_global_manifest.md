# Pixi Global Manifest

!!! tip "Feedback wanted"

    This document is work in progress, and community feedback is greatly appreciated.
    Please share your thoughts at our [GitHub discussion](https://github.com/prefix-dev/pixi/discussions/1799).

## Motivation

`pixi global` is currently limited to imperatively managing CLI packages.
The next iteration of this feature should fulfill the following needs:

- Shareable global environments.
- Managing complex environments with multiple packages as dependencies
- Flexible exposure of executables

## Design Considerations

There are a few things we wanted to keep in mind in the design:

1. **User-friendliness**: Pixi is a user focused tool that goes beyond developers. The feature should have good error reporting and helpful documentation from the start.
2. **Keep it simple**: The CLI should be all you strictly need to interact with global environments.
3. **Unsurprising**: Simple commands should behave similar to traditional package managers.
4. **Human Readable**: Any file created by this feature should be human-readable and modifiable.

## Manifest

The global environments and exposed will be managed by a human-readable manifest.
This manifest will stick to conventions set by `pixi.toml` where possible.
Among other things it will be written in the TOML format, be named `pixi-global.toml` and be placed at `~/.pixi/manifests/pixi-global.toml`.
The motivation for the location is discussed [further below](#multiple-manifests)

```toml title="pixi-global.toml"
# The name of the environment is `python`
[envs.python]
channels = ["conda-forge"]
# optional, defaults to your current OS
platform = "osx-64"
# It will expose python, python3 and python3.11, but not pip
[envs.python.dependencies]
python = "3.11.*"
pip = "*"

[envs.python.exposed]
python = "python"
python3 = "python3"
"python3.11" = "python3.11"

# The name of the environment is `python3-10`
[envs.python3-10]
channels = ["https://fast.prefix.dev/conda-forge"]
# It will expose python3.10
[envs.python3-10.dependencies]
python = "3.10.*"

[envs.python3-10.exposed]
"python3.10" = "python"
```

## CLI

Install one or more packages `PACKAGE` and expose their executables.
If `--environment` has been given, all packages will be installed in the same environment.
`--expose` can be given if `--environment` is given as well or if only a single `PACKAGE` will be installed.
The syntax for `MAPPING` is `exposed_name=executable_name`, so for example `python3.10=python`.
`--platform` sets the platform of the environment to `PLATFORM`
Multiple channels can be specified by using `--channel` multiple times.
By default, if no channel is provided, the `default-channels` key in the pixi configuration is used, which again defaults to "conda-forge".

```
pixi global install [--expose MAPPING] [--environment ENV] [--platform PLATFORM] [--no-activation] [--channel CHANNEL]... PACKAGE...
```

Remove environments `ENV`.
```
pixi global uninstall <ENV>...
```

Update `PACKAGE` if `--package` is given. If not, all packages in environments `ENV` will be updated.
If the update leads to executables being removed, it will offer to remove the mappings.
If the user declines the update process will stop.
If the update leads to executables being added, it will offer for each binary individually to expose it.
```
pixi global update [--package PACKAGE] <ENV>...
```

Updates all packages in all environments.
If the update leads to executables being removed, it will offer to remove the mappings.
If the user declines the update process will stop.
If the update leads to executables being added, it will offer for each binary individually to expose it.
`--assume-yes` will assume yes as answer for every question that would otherwise be asked interactively.

```
pixi global update-all [--assume-yes]
```

Add one or more packages `PACKAGE` into an existing environment `ENV`.
If environment `ENV` does not exist, it will return with an error.
Without `--expose` no binary will be exposed.
If you don't mention a spec like `python=3.8.*`, the spec will be unconstrained with `*`.
The syntax for `MAPPING` is `exposed_name=executable_name`, so for example `python3.10=python`.

```
pixi global add --environment ENV [--expose MAPPING] <PACKAGE>...
```

Remove package `PACKAGE` from environment `ENV`.
If that was the last package remove the whole environment and print that information in the console.
If this leads to executables being removed, it will offer to remove the mappings.
If the user declines the remove process will stop.
```
pixi global remove --environment ENV PACKAGE
```

Add one or more `MAPPING` for environment `ENV` which describe which executables are exposed.
The syntax for `MAPPING` is `exposed_name=executable_name`, so for example `python3.10=python`.
```
pixi global expose add --environment ENV  <MAPPING>...
```

Remove one or more exposed `BINARY` from environment `ENV`
```
pixi global expose remove --environment ENV <BINARY>...
```

Ensure that the environments on the machine reflect the state in the manifest.
The manifest is the single source of truth.
Only if there's no manifest, will the data from existing environments be used to create a manifest.
`pixi global sync` is implied by most other `pixi global` commands.

```
pixi global sync
```

List all environments, their specs and exposed executables
```
pixi global list
```

Set the channels `CHANNEL` for a certain environment `ENV` in the pixi global manifest.
```
pixi global channel set --environment ENV <CHANNEL>...
```

Set the platform `PLATFORM` for a certain environment `ENV` in the pixi global manifest.
```
pixi global platform set --environment ENV PLATFORM
```


### Simple workflow

Create environment `python`, install package `python=3.10.*` and expose all executables of that package
```
pixi global install python=3.10.*
```

Update all packages in environment `python`
```
pixi global update python
```

Remove environment `python`
```
pixi global uninstall python
```

Create environment `python` and `pip`, install corresponding packages and expose all executables of that packages
```
pixi global install python pip
```

Remove environments `python` and `pip`
```
pixi global uninstall python pip
```

Create environment `python-pip`, install `python` and `pip` in the same environment and expose all executables of these packages
```
pixi global install --environment python-pip python pip
```


### Adding dependencies

Create environment `python`, install package `python` and expose all executables of that package.
Then add package `hypercorn` to environment `python` but doesn't expose its executables.

```
pixi global install python
pixi global add --environment python hypercorn
```

Update package `cryptography` (a dependency of `hypercorn`) to `43.0.0` in environment `python`

```
pixi update --environment python cryptography=43.0.0
```

Then remove `hypercorn` again.
```
pixi global remove --environment python hypercorn
```


### Specifying which executables to expose

Make a new environment `python3-10` with package `python=3.10` and expose the `python` executable as `python3.10`.
```
pixi global install --environment python3-10 --expose "python3.10=python" python=3.10
```

Now `python3.10` is available.


Run the following in order to expose `python` from environment `python3-10` as `python3-10` instead.

```
pixi global expose remove --environment python3-10 python3.10
pixi global expose add --environment python3-10 "python3-10=python"
```

Now `python3-10` is available, but `python3.10` isn't anymore.


### Syncing

Most `pixi global` sub commands imply a `pixi global sync`.

- Users should be able to change the manifest by hand (creating or modifying (adding or removing))
- Users should be able to "export" their existing environments into the manifest, if non-existing.
- The manifest is always "in sync" after `install`/`remove`/`inject`/`other global command`.


First time, clean computer.
Running the following creates manifest and `~/.pixi/envs/python`.
```
pixi global install python
```

Delete `~/.pixi` and syncing, should add environment `python` again as described in the manifest
```
rm `~/.pixi/envs`
pixi global sync
```

If there's no manifest, but existing environments, pixi will create a manifest that matches your current environments.
It is to be decided whether the user should be asked if they want an empty manifest instead, or if it should always import the data from the environments.
```
rm <manifest>
pixi global sync
```

If we remove the python environment from the manifest, running `pixi global sync` will also remove the `~/.pixi/envs/python` environment from the file system.
```
vim <manifest>
pixi global sync
```

## Open Questions

### Should we version the manifest?

Something like:

```
[manifest]
version = 1
```

We still have to figure out which existing programs do something similar and how they benefit from it.

### Multiple manifests

We could go for one default manifest, but also parse other manifests in the same directory.
The only requirement to be parsed as manifest is a `.toml` extension
In order to modify those with the `CLI` one would have to add an option `--manifest` to select the correct one.

- pixi-global.toml: Default
- pixi-global-company-tools.toml
- pixi-global-from-my-dotfiles.toml

It is unclear whether the first implementation already needs to support this.
At the very least we should put the manifest into its own folder like `~/.pixi/global/manifests/pixi-global.toml`


### Discovery via config key

In order to make it easier to manage manifests in version control, we could allow to set the manifest path via a key in the [pixi configuration](https://pixi.sh/dev/reference/pixi_configuration/).


``` title="config.toml"
global_manifests = "/path/to/your/manifests"
```


### No activation

The current `pixi global install` features `--no-activation`.
When this flag is set, `CONDA_PREFIX` and `PATH` will not be set when running the exposed executable.
This is useful when installing Python package managers or shells.

Assuming that this needs to be set per mapping, one way to expose this functionality would be to allow the following:

```toml
[envs.pip.exposed]
pip = { executable="pip", activation=false }
```
