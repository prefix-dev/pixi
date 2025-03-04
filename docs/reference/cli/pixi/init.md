--8<-- [start:description]

!!! info "Importing an environment.yml"
    When importing an environment, the `pixi.toml` will be created with the dependencies from the environment file.
    The `pixi.lock` will be created when you install the environment.
    We don't support `git+` urls as dependencies for pip packages and for the `defaults` channel we use `main`, `r` and `msys2` as the default channels.


--8<-- [end:description]


--8<-- [start:example]

## Examples

```shell
pixi init myproject
pixi init ~/myproject
pixi init  # Initializes directly in the current directory.
pixi init --channel conda-forge --channel bioconda myproject
pixi init --platform osx-64 --platform linux-64 myproject
pixi init --import environment.yml
pixi init --format pyproject
pixi init --format pixi --scm gitlab
```

--8<-- [end:example]
