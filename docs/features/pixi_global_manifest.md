# Pixi Global Manifest

### Motivation

`pixi global` is currently limited to imperatively managing CLI packages.
The next iteration of this feature should fulfill the following needs:

- Shareable global environments.
- Allow managing complex environments with multiple packages as dependencies
- Flexible exposure of binaries

## Design Considerations

There are a few things we wanted to keep in mind in the design:

1. **User-friendliness**: Pixi is a user focussed tool that goes beyond developers. The feature should have good error reporting and helpful documentation from the start.
2. **Keep it simple**: The CLI should be all you need to interact with the global environments.
3. **Unsurprising**: Simple commands should still behave similar to traditional package managers.
4. **Human Readable**: Any file created by this feature should be human-readable and modifiable.

## Manifest

```toml title="pixi-global.toml"
# `python_3_10` is the name of the environment
[envs.python_3_10.dependencies.python]
spec = "3.10.*"
expose_binaries = {"python3.10"="python"} # `python` from `python_3_10` will be available as `python3.10`

[envs.python.dependencies.python]
spec = "3.11.*"
expose_binaries = "auto" # This will expose python, python3 and python3.11

[envs.python.dependencies.pip]
spec = "*"
```

## CLI

Install one or more packages `PACKAGE` into their own environments and expose their binaries

```
pixi global install [PACKAGE]...
```

Remove environments of the named packages `PACKAGE`
```
pixi global uninstall [PACKAGE]...
```

Resolve environment without previously defined specs
```
pixi global upgrade python
```

Add package `PACKAGE` to environment `ENV`.
If environment `ENV` does not yet exist, then it will be created.
```
pixi global add --name ENV PACKAGE
```

Remove package `PACKAGE` from environment `ENV`.
```
pixi global remove --name ENV PACKAGE
```

Update the version of one package
```
pixi global update --name python python=3.12.*
```

Set for a specific package `PACKAGE` in environment `ENV` under which `MAPPING` binaries are exposed
```
pixi expose-bin --name ENV --package PACKAGE [MAPPING]...
```

Ensure that the environments on the machine reflect the state in the manifest
```
pixi global sync
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
pixi global remove python
```

Create environment `python` and `pip`, install corresponding packages and expose all binaries of that packages
```
pixi global install python pip
```

Remove environments `python` and `pip`
```
pixi global remove python pip
```

### Injecting dependencies

Create environment `python`, install package `python` and expose all binaries of that package.
Then add package `hypercorn` to environment `python` but doesn't expose its binaries.

```
pixi global install python
pixi global add --name=python hypercorn
```

Update package `cryptography` (a dependency of `hypercorn`) in environment `python`

```
pixi update --name python cryptography
```

Then remove `hypercorn` again.
```
pixi global remove --name=python hypercorn
```


### Specifying which binaries to expose

Make a new environment `python_3_10` with package `python=3.10` and no exposed binaries
```
pixi global add --name python_3_10 python=3.10
```

Expose `python` from environment `python_3_10` as `python3.10`.

```
pixi expose-bin --name python_3_10 --package python "python3.10=python"
```
