# Multi Environment Support

### Motivating Example

There are multiple scenarios where multiple environments are useful.

- **Testing of multiple package versions**, e.g. `py39` and `py310` or polars `0.12` and `0.13`.
- **Smaller single tool environments**, e.g. `lint` or `docs`.
- **Large developer environments**, that combine all the smaller environments, e.g. `dev`.
- **Strict supersets of environments**, e.g. `prod` and `test-prod` where `test-prod` is a strict superset of `prod`.
- **Multiple machines from one project**, e.g. a `cuda` environment and a `cpu` environment.
- **And many more.** (Feel free to edit this document in our GitHub and add your use case.)

This prepares `pixi` for use in large projects with multiple use-cases, multiple developers and different CI needs.

## Design Considerations

There are a few things we wanted to keep in mind in the design:

1. **User-friendliness**: Pixi is a user focussed tool that goes beyond developers. The feature should have good error reporting and helpful documentation from the start.
2. **Keep it simple**: Not understanding the multiple environments feature shouldn't limit a user to use pixi. The feature should be "invisible" to the non-multi env use-cases.
3. **No Automatic Combinatorial**: To ensure the dependency resolution process remains manageable, the solution should avoid a combinatorial explosion of dependency sets. By making the environments user defined and not automatically inferred by testing a matrix of the features.
4. **Single environment Activation**: The design should allow only one environment to be active at any given time, simplifying the resolution process and preventing conflicts.
5. **Fixed lock files**: It's crucial to preserve fixed lock files for consistency and predictability. Solutions must ensure reliability not just for authors but also for end-users, particularly at the time of lock file creation.

### Feature & Environment Set Definitions

Introduce environment sets into the `pixi.toml` this describes environments based on `feature`'s. Introduce features into the `pixi.toml` that can describe parts of environments.
As an environment goes beyond just `dependencies` the `features` should be described including the following fields:

- `dependencies`: The conda package dependencies
- `pypi-dependencies`: The pypi package dependencies
- `system-requirements`: The system requirements of the environment
- `activation`: The activation information for the environment
- `platforms`: The platforms the environment can be run on.
- `channels`: The channels used to create the environment. Adding the `priority` field to the channels to allow concatenation of channels instead of overwriting.
- `target`: All the above features but also separated by targets.
- `tasks`: Feature specific tasks, tasks in one environment are selected as default tasks for the environment.

```toml title="Default features"
[dependencies] # short for [feature.default.dependencies]
python = "*"
numpy = "==2.3"

[pypi-dependencies] # short for [feature.default.pypi-dependencies]
pandas = "*"

[system-requirements] # short for [feature.default.system-requirements]
libc = "2.33"

[activation] # short for [feature.default.activation]
scripts = ["activate.sh"]
```

```toml title="Different dependencies per feature"
[feature.py39.dependencies]
python = "~=3.9.0"
[feature.py310.dependencies]
python = "~=3.10.0"
[feature.test.dependencies]
pytest = "*"
```

```toml title="Full set of environment modification in one feature"
[feature.cuda]
dependencies = {cuda = "x.y.z", cudnn = "12.0"}
pypi-dependencies = {torch = "1.9.0"}
platforms = ["linux-64", "osx-arm64"]
activation = {scripts = ["cuda_activation.sh"]}
system-requirements = {cuda = "12"}
# Channels concatenate using a priority instead of overwrite, so the default channels are still used.
# Using the priority the concatenation is controlled, default is 0, the default channels are used last.
# Highest priority comes first.
channels = ["nvidia", {channel = "pytorch", priority = -1}] # Results in:  ["nvidia", "conda-forge", "pytorch"] when the default is `conda-forge`
tasks = { warmup = "python warmup.py" }
target.osx-arm64 = {dependencies = {mlx = "x.y.z"}}
```

```toml title="Define tasks as defaults of an environment"
[feature.test.tasks]
test = "pytest"

[environments]
test = ["test"]

# `pixi run test` == `pixi run --environment test test`
```

The environment definition should contain the following fields:

- `features: Vec<Feature>`: The features that are included in the environment set, which is also the default field in the environments.
- `solve-group: String`: The solve group is used to group environments together at the solve stage.
  This is useful for environments that need to have the same dependencies but might extend them with additional dependencies.
  For instance when testing a production environment with additional test dependencies.

```toml title="Creating environments from features"
[environments]
# implicit: default = ["default"]
default = ["py39"] # implicit: default = ["py39", "default"]
py310 = ["py310"] # implicit: py310 = ["py310", "default"]
test = ["test"] # implicit: test = ["test", "default"]
test39 = ["test", "py39"] # implicit: test39 = ["test", "py39", "default"]
```

```toml title="Testing a production environment with additional dependencies"
[environments]
# Creating a `prod` environment which is the minimal set of dependencies used for production.
prod = {features = ["py39"], solve-group = "prod"}
# Creating a `test_prod` environment which is the `prod` environment plus the `test` feature.
test_prod = {features = ["py39", "test"], solve-group = "prod"}
# Using the `solve-group` to solve the `prod` and `test_prod` environments together
# Which makes sure the tested environment has the same version of the dependencies as the production environment.
```

```toml title="Creating environments without including the default feature"
[dependencies]
python = "*"
numpy = "*"

[feature.lint.dependencies]
pre-commit = "*"

[environments]
# Create a custom environment which only has the `lint` feature (numpy isn't part of that env).
lint = {features = ["lint"], no-default-feature = true}
```

### lock file Structure

Within the `pixi.lock` file, a package may now include an additional `environments` field, specifying the environment to which it belongs.
To avoid duplication the packages `environments` field may contain multiple environments so the lock file is of minimal size.

```yaml
- platform: linux-64
  name: pre-commit
  version: 3.3.3
  category: main
  environments:
    - dev
    - test
    - lint
  ...:
- platform: linux-64
  name: python
  version: 3.9.3
  category: main
  environments:
    - dev
    - test
    - lint
    - py39
    - default
  ...:
```

### User Interface Environment Activation

Users can manually activate the desired environment via command line or configuration.
This approach guarantees a conflict-free environment by allowing only one feature set to be active at a time.
For the user the cli would look like this:

```shell title="Default behavior"
➜ pixi run python
# Runs python in the `default` environment
```

```shell title="Activating an specific environment"
➜ pixi run -e test pytest
➜ pixi run --environment test pytest
# Runs `pytest` in the `test` environment
```

```shell title="Activating a shell in an environment"
➜ pixi shell -e cuda
pixi shell --environment cuda
# Starts a shell in the `cuda` environment
```

```shell title="Running any command in an environment"
➜ pixi run -e test any_command
# Runs any_command in the `test` environment which doesn't require to be predefined as a task.
```
### Ambiguous Environment Selection
It's possible to define tasks in multiple environments, in this case the user should be prompted to select the environment.

Here is a simple example of a task only manifest:

```toml title="pixi.toml"
[project]
name = "test_ambiguous_env"
channels = []
platforms = ["linux-64", "win-64", "osx-64", "osx-arm64"]

[tasks]
default = "echo Default"
ambi = "echo Ambi::Default"
[feature.test.tasks]
test = "echo Test"
ambi = "echo Ambi::Test"

[feature.dev.tasks]
dev = "echo Dev"
ambi = "echo Ambi::Dev"

[environments]
default = ["test", "dev"]
test = ["test"]
dev = ["dev"]
```
Trying to run the `abmi` task will prompt the user to select the environment.
As it is available in all environments.

```shell title="Interactive selection of environments if task is in multiple environments"
➜ pixi run ambi
? The task 'ambi' can be run in multiple environments.

Please select an environment to run the task in: ›
❯ default # selecting default
  test
  dev

✨ Pixi task (ambi in default): echo Ambi::Test
Ambi::Test
```

As you can see it runs the task defined in the `feature.task` but it is run in the `default` environment.
This happens because the `ambi` task is defined in the `test` feature, and it is overwritten in the default environment.
So the `tasks.default` is now non-reachable from any environment.

Some other results running in this example:
```shell
➜ pixi run --environment test ambi
✨ Pixi task (ambi in test): echo Ambi::Test
Ambi::Test

➜ pixi run --environment dev ambi
✨ Pixi task (ambi in dev): echo Ambi::Dev
Ambi::Dev

# dev is run in the default environment
➜ pixi run dev
✨ Pixi task (dev in default): echo Dev
Dev

# dev is run in the dev environment
➜ pixi run -e dev dev
✨ Pixi task (dev in dev): echo Dev
Dev
```


## Important links

- Initial writeup of the proposal: [GitHub Gist by 0xbe7a](https://gist.github.com/0xbe7a/bbf8a323409be466fe1ad77aa6dd5428)
- GitHub project: [#10](https://github.com/orgs/prefix-dev/projects/10)

## Real world example use cases

??? tip "Polarify test setup"

    In `polarify` they want to test multiple versions combined with multiple versions of polars.
    This is currently done by using a matrix in GitHub actions.
    This can be replaced by using multiple environments.

    ```toml title="pixi.toml"
    [project]
    name = "polarify"
    # ...
    channels = ["conda-forge"]
    platforms = ["linux-64", "osx-arm64", "osx-64", "win-64"]

    [tasks]
    postinstall = "pip install --no-build-isolation --no-deps --disable-pip-version-check -e ."

    [dependencies]
    python = ">=3.9"
    pip = "*"
    polars = ">=0.14.24,<0.21"

    [feature.py39.dependencies]
    python = "3.9.*"
    [feature.py310.dependencies]
    python = "3.10.*"
    [feature.py311.dependencies]
    python = "3.11.*"
    [feature.py312.dependencies]
    python = "3.12.*"
    [feature.pl017.dependencies]
    polars = "0.17.*"
    [feature.pl018.dependencies]
    polars = "0.18.*"
    [feature.pl019.dependencies]
    polars = "0.19.*"
    [feature.pl020.dependencies]
    polars = "0.20.*"

    [feature.test.dependencies]
    pytest = "*"
    pytest-md = "*"
    pytest-emoji = "*"
    hypothesis = "*"
    [feature.test.tasks]
    test = "pytest"

    [feature.lint.dependencies]
    pre-commit = "*"
    [feature.lint.tasks]
    lint = "pre-commit run --all"

    [environments]
    pl017 = ["pl017", "py39", "test"]
    pl018 = ["pl018", "py39", "test"]
    pl019 = ["pl019", "py39", "test"]
    pl020 = ["pl020", "py39", "test"]
    py39 = ["py39", "test"]
    py310 = ["py310", "test"]
    py311 = ["py311", "test"]
    py312 = ["py312", "test"]
    ```

    ```yaml title=".github/workflows/test.yml"
    jobs:
      tests-per-env:
        runs-on: ubuntu-latest
        strategy:
          matrix:
            environment: [py311, py312]
        steps:
        - uses: actions/checkout@v4
          - uses: prefix-dev/setup-pixi@v0.5.1
            with:
              environments: ${{ matrix.environment }}
          - name: Run tasks
            run: |
              pixi run --environment ${{ matrix.environment }} test
      tests-with-multiple-envs:
        runs-on: ubuntu-latest
        steps:
        - uses: actions/checkout@v4
        - uses: prefix-dev/setup-pixi@v0.5.1
          with:
           environments: pl017 pl018
        - run: |
            pixi run -e pl017 test
            pixi run -e pl018 test
    ```

??? tip "Test vs Production example"

    This is an example of a project that has a `test` feature and `prod` environment.
    The `prod` environment is a production environment that contains the run dependencies.
    The `test` feature is a set of dependencies and tasks that we want to put on top of the previously solved `prod` environment.
    This is a common use case where we want to test the production environment with additional dependencies.

    ```toml title="pixi.toml"
    [project]
    name = "my-app"
    # ...
    channels = ["conda-forge"]
    platforms = ["osx-arm64", "linux-64"]

    [tasks]
    postinstall-e = "pip install --no-build-isolation --no-deps --disable-pip-version-check -e ."
    postinstall = "pip install --no-build-isolation --no-deps --disable-pip-version-check ."
    dev = "uvicorn my_app.app:main --reload"
    serve = "uvicorn my_app.app:main"

    [dependencies]
    python = ">=3.12"
    pip = "*"
    pydantic = ">=2"
    fastapi = ">=0.105.0"
    sqlalchemy = ">=2,<3"
    uvicorn = "*"
    aiofiles = "*"

    [feature.test.dependencies]
    pytest = "*"
    pytest-md = "*"
    pytest-asyncio = "*"
    [feature.test.tasks]
    test = "pytest --md=report.md"

    [environments]
    # both default and prod will have exactly the same dependency versions when they share a dependency
    default = {features = ["test"], solve-group = "prod-group"}
    prod = {features = [], solve-group = "prod-group"}
    ```
    In ci you would run the following commands:
    ```shell
    pixi run postinstall-e && pixi run test
    ```
    Locally you would run the following command:
    ```shell
    pixi run postinstall-e && pixi run dev
    ```

    Then in a Dockerfile you would run the following command:
    ```dockerfile title="Dockerfile"
    FROM ghcr.io/prefix-dev/pixi:latest # this doesn't exist yet
    WORKDIR /app
    COPY . .
    RUN pixi run --environment prod postinstall
    EXPOSE 8080
    CMD ["/usr/local/bin/pixi", "run", "--environment", "prod", "serve"]
    ```

??? tip "Multiple machines from one project"
    This is an example for an ML project that should be executable on a machine that supports `cuda` and `mlx`. It should also be executable on machines that don't support `cuda` or `mlx`, we use the `cpu` feature for this.

    ```toml title="pixi.toml"
    [project]
    name = "my-ml-project"
    description = "A project that does ML stuff"
    authors = ["Your Name <your.name@gmail.com>"]
    channels = ["conda-forge", "pytorch"]
    # All platforms that are supported by the project as the features will take the intersection of the platforms defined there.
    platforms = ["win-64", "linux-64", "osx-64", "osx-arm64"]

    [tasks]
    train-model = "python train.py"
    evaluate-model = "python test.py"

    [dependencies]
    python = "3.11.*"
    pytorch = {version = ">=2.0.1", channel = "pytorch"}
    torchvision = {version = ">=0.15", channel = "pytorch"}
    polars = ">=0.20,<0.21"
    matplotlib-base = ">=3.8.2,<3.9"
    ipykernel = ">=6.28.0,<6.29"

    [feature.cuda]
    platforms = ["win-64", "linux-64"]
    channels = ["nvidia", {channel = "pytorch", priority = -1}]
    system-requirements = {cuda = "12.1"}

    [feature.cuda.tasks]
    train-model = "python train.py --cuda"
    evaluate-model = "python test.py --cuda"

    [feature.cuda.dependencies]
    pytorch-cuda = {version = "12.1.*", channel = "pytorch"}

    [feature.mlx]
    platforms = ["osx-arm64"]
    # MLX is only available on macOS >=13.5 (>14.0 is recommended)
    system-requirements = {macos = "13.5"}

    [feature.mlx.tasks]
    train-model = "python train.py --mlx"
    evaluate-model = "python test.py --mlx"

    [feature.mlx.dependencies]
    mlx = ">=0.16.0,<0.17.0"

    [feature.cpu]
    platforms = ["win-64", "linux-64", "osx-64", "osx-arm64"]

    [environments]
    cuda = ["cuda"]
    mlx = ["mlx"]
    default = ["cpu"]
    ```

    ```shell title="Running the project on a cuda machine"
    pixi run train-model --environment cuda
    # will execute `python train.py --cuda`
    # fails if not on linux-64 or win-64 with cuda 12.1
    ```

    ```shell title="Running the project with mlx"
    pixi run train-model --environment mlx
    # will execute `python train.py --mlx`
    # fails if not on osx-arm64
    ```

    ```shell title="Running the project on a machine without cuda or mlx"
    pixi run train-model
    ```
