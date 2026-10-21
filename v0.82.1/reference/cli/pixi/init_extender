--8<-- [start:description]

!!! info "Importing an environment.yml"
    When importing an environment, the `pixi.toml` will be created with the dependencies from the environment file.
    The `pixi.lock` will be created when you install the environment.
    We don't support `git+` urls as dependencies for pip packages and for the `defaults` channel we use `main`, `r` and `msys2` as the default channels.


--8<-- [end:description]


--8<-- [start:example]

## Examples

```shell
pixi init myproject  # (1)!
pixi init ~/myproject  # (2)!
pixi init  # (3)!
pixi init --channel conda-forge --channel bioconda myproject  # (4)!
pixi init --platform osx-64 --platform linux-64 myproject  # (5)!
pixi init --import environment.yml  # (6)!
pixi init --format pyproject  # (7)!
pixi init --format pixi --scm gitlab  # (8)!
```

1. Initializes a new project in the `myproject` directory, relative to the current directory.
2. Initializes a new project in the `~/myproject` directory, absolute path.
3. Initializes a new project in the current directory.
4. Initializes a new project with the specified channels.
5. Initializes a new project with the specified platforms.
6. Initializes a new project with the `dependencies` and `channels` from the `environment.yml` file.
7. Initializes a new project with the `pyproject.toml` format.
8. Initializes a new project with the `pixi.toml` format and the `gitlab` SCM.

--8<-- [end:example]
