# Pixi Global Manifest

## Motivation

`pixi global` is currently limited to imperatively managing CLI packages.
The next iteration of this feature should fulfill the following needs:

- Shareable global environments.
- Managing complex environments with multiple packages as dependencies
- Flexible exposure of binaries

## Design Considerations

There are a few things we wanted to keep in mind in the design:

1. **User-friendliness**: Pixi is a user focused tool that goes beyond developers. The feature should have good error reporting and helpful documentation from the start.
2. **Keep it simple**: The CLI should be all you strictly need to interact with global environments.
3. **Unsurprising**: Simple commands should behave similar to traditional package managers.
4. **Human Readable**: Any file created by this feature should be human-readable and modifiable.

## Manifest

The global environments and exposed binaries will be managed by a human-readable manifest.
This manifest will stick to conventions set by `pixi.toml` where possible.
Among other things it will be written in the TOML format, be named `pixi-global.toml` and be placed at `~/.pixi/manifests/pixi-global.toml`.
The motivation for the location is discussed [further below](#multiple-manifests)

```toml title="pixi-global.toml"
# The name of the environment is `python`
# If the `envs.ENV.expose` key is missing, all binaries of `envs.ENV.package` will be exposed
# It will expose python, python3 and python3.11, but not pip
[envs.python]
package = { python = "3.11.*" }

[envs.python.injected]
pip = "*"

# The name of the environment is `python_3_10`
# It will expose python as python3.10
[envs.python_3_10]
package = { python = "3.10.*" }

[envs.python_3_10.expose]
"python3.10"="python"
```

## CLI

Install one or more packages `PACKAGE` into their own environments (`ENV` if given by `--environment` or else `PACKAGE`) and expose their binaries.
The syntax for `MAPPING` is `exposed_name=binary_in_env`, so for example `python3.10=python`.
If `binary_in_env` doesn't come from `env.ENV.package` it will error out.
If the environment doesn't exist, it will be created.
If the environment does exist, and
- the package is the same, set the package version to the given spec
- the package is not the same, error out

If `--environment` or `--expose` and more than one `<PACKAGE>` are given, error out.

In the manifest, `env.ENV.expose` will only be set if `--expose` is given.

```
pixi global install [--environment ENV] [--expose MAPPING] <PACKAGE>...
```

Remove environments `ENV`.
```
pixi global uninstall [ENV]...
```

Update all packages in environments `ENV`.
The manifest will not be modified, the given specs are respected.
```
pixi global update [ENV]...
```

Inject package `PACKAGE` into an existing environment `ENV`.
If environment `ENV` does not exist, it will return with an error.
If `PACKAGE` already exists it will set it to the given spec
In the manifest, `PACKAGE` will be added to `env.ENV.injected`.

```
pixi global inject --environment ENV PACKAGE
```

Remove package `PACKAGE` from environment `ENV`.
In the manifest, `PACKAGE` will be removed from `env.ENV.injected`.
```
pixi global uninject --environment ENV PACKAGE
```

Set for a specific environment `ENV` under which `MAPPING` binaries are exposed
The syntax for `MAPPING` is `exposed_name=binary_in_env`, so for example `python3.10=python`.
If `binary_in_env` doesn't come from `env.ENV.package` it will error out.
In the manifest, existing mappings will be removed
```
pixi expose --environment ENV  [MAPPING]...
```

Ensure that the environments on the machine reflect the state in the manifest.
The manifest is the single source of truth.
Only if there's no manifest, will the data from existing environments be used to create a manifest.
`pixi global sync` is implied by most other `pixi global` commands.

```
pixi global sync
```

List all environments, their specs and exposed binaries
```
pixi global list
```


### Simple workflow

Create environment `python`, install package `python=3.10.*` and expose all binaries of that package
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

Create environment `python` and `pip`, install corresponding packages and expose all binaries of that packages
```
pixi global install python pip
```

Remove environments `python` and `pip`
```
pixi global uninstall python pip
```

### Injecting dependencies

Create environment `python`, install package `python` and expose all binaries of that package.
Then inject package `hypercorn` into environment `python` but doesn't expose its binaries.

```
pixi global install python
pixi global inject --environment=python hypercorn
```

Update package `cryptography` (a dependency of `hypercorn`) in environment `python`

```
pixi update --environment python cryptography
```

Then uninject `hypercorn` again.
```
pixi global uninject --environment=python hypercorn
```


### Specifying which binaries to expose

Make a new environment `python_3_10` with package `python=3.10` and expose the `python` executable as `python3.10`.
```
pixi global install --environment python_3_10 --expose "python3.10=python" python=3.10
```

Now `python3.10` is available.


Run the following in order to expose `python` from environment `python_3_10` as `python310` instead.

```
pixi global expose --environment python_3_10 "python310=python"
```

Now `python310` is available, but `python3.10` isn't anymore.

### Syncing

Most `pixi global` sub commands imply a `pixi global sync`.

- Users should be able to change the manifest by hand
- Users should be able to "export" their existing environments into the manifest, if non-existing
- The manifest is always be in sync after running any global command


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
In order to modify those with the `CLI` one would have to add an option `--manifest` to select the correct one.

- pixi-global.toml: Default
- pixi-global-company-tools.toml
- pixi-global-from-my-dotfiles.toml

It is unclear whether the first implementation already needs to support this.
At the very least we should put the manifest into its own folder like `~/.pixi/global/manifests/pixi-global.toml`
