# Pixi Global Manifest

### Motivation

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

```toml title="pixi-global.toml"
# `python_3_10` is the name of the environment
[envs.python_3_10.dependencies]
# `python` from `python_3_10` will be available as `python3.10`
python = { spec = "3.10.*", expose_binaries = { "python3.10"="python" } }

[envs.python.dependencies]
 # This will expose python, python3 and python3.11
python = { spec = "3.11.*", expose_binaries = "auto" }
pip = "*"
```

## CLI

Install one or more packages `PACKAGE` into their own environments and expose their binaries.
If no environment named `PACKAGE` exists, it will be created.
The syntax for `MAPPING` is `exposed_name=binary_name`, so for example `python3.10=python`.

```
pixi global install --expose MAPPING [PACKAGE]...
```

Remove environments `ENV`.
```
pixi global uninstall [ENV]...
```

Upgrade all packages in environments `ENV`
```
pixi global upgrade [ENV]...
```

Inject package `PACKAGE` into an existing environment `ENV`.
If environment `ENV` does not exist, it will return with an error.
```
pixi global inject --environment ENV PACKAGE
```

Remove package `PACKAGE` from environment `ENV`.
```
pixi global remove --environment ENV PACKAGE
```

Update the version of one package
```
pixi global update --environment ENV PACKAGE
```

Set for a specific package `PACKAGE` in environment `ENV` under which `MAPPING` binaries are exposed
The syntax for `MAPPING` is `exposed_name=binary_name`, so for example `python3.10=python`.
```
pixi expose-bin --environment ENV --package PACKAGE [MAPPING]...
```

Ensure that the environments on the machine reflect the state in the manifest.
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

Upgrade all packages in environment `python`
```
pixi global upgrade python
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
Then add package `hypercorn` to environment `python` but doesn't expose its binaries.

```
pixi global install python
pixi global inject --environment=python hypercorn
```

Update package `cryptography` (a dependency of `hypercorn`) in environment `python`

```
pixi update --environment python cryptography
```

Then remove `hypercorn` again.
```
pixi global remove --environment=python hypercorn
```


### Specifying which binaries to expose

Make a new environment `python_3_10` with package `python=3.10` and no exposed binaries
```
pixi global install --environment python_3_10 --expose "python3.10=python" python=3.10
```

Expose `python` from environment `python_3_10` as `python310` instead.

```
pixi expose-bin --environment python_3_10 --package python "python310=python"
```


## Behavior

### How to behave when no manifest exists?

Every time a `pixi global` command is executed, it checks if manifest exists.
If not, it should offer to create one.
If there are already environments on the system, it should offer to create a manifest that matches the existing environments as close as possible.


### Multiple manifests

We could go for one default manifest, but also parse other manifests in the same directory.
In order to modify those with the `CLI` one would have to add an option `--manifest` to select the correct one.

- pixi-global.toml: Default
- pixi-global-company-tools.toml
- pixi-global-from-my-dotfiles.toml
