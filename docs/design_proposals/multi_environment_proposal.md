# Proposal Design: Multi Environment Support
## Objective
The aim is to introduce an environment set mechanism in the `pixi` package manager.
This mechanism will enable clear, conflict-free management of dependencies tailored to specific environments, while also maintaining the integrity of fixed lockfiles.


### Motivating Example
There are multiple scenarios where multiple environments are useful.

- **Testing of multiple package versions**, e.g. `py39` and `py310` or polars `0.12` and `0.13`.
- **Smaller single tool environments**, e.g. `lint` or `docs`.
- **Large developer environments**, that combine all the smaller environments, e.g. `dev`.
- **Strict supersets of environments**, e.g. `prod` and `test-prod` where `test-prod` is a strict superset of `prod`.
- **Multiple machines from one project**, e.g. a `cuda` environment and a `cpu` environment.
- **And many more.** (If you have a use-case please add it to the list, so we can make sure it's covered)

This prepares `pixi` for the use in large projects with multiple use-cases, multiple developers and different CI needs.

## Design Considerations
1. **User-friendliness**: Pixi is a user focussed tool this goes beyond developers. The feature should have good error reporting and helpful documentation from the start. This is opinionated so the user sided PR's should be checked by multiple developers.
2. **Keep it simple**: Not understanding the multiple environments feature shouldn't limit a user to use pixi. The feature should be "invisible" to the non-multi env use-cases.
3. **No Automatic Combinatorial**: To ensure the dependency resolution process remains manageable, the solution should avoid a combinatorial explosion of dependency sets. By making the environments user defined and not automatically inferred by testing a matrix of the features.
4. **Single environment Activation**: The design should allow only one environment to be active at any given time, simplifying the resolution process and preventing conflicts.
5. **Fixed Lockfiles**: It's crucial to preserve fixed lockfiles for consistency and predictability. Solutions must ensure reliability not just for authors but also for end-users, particularly at the time of lockfile creation.

## Proposed Solution
!!! important
    This is a proposal, not a final design. The proposal is open for discussion and will be updated based on the feedback.

### Feature & Environment Set Definitions
Introduce environment sets into the `pixi.toml` this describes environments based on `feature`'s. Introduce features into the `pixi.toml` that can describe parts of environments.
As an environment goes beyond just `dependencies` the `features` should be described including the following fields:

- `dependencies`: The conda package dependencies
- `pypi-dependencies`: The pypi package dependencies
- `system-requirements`: The system requirements of the environment
- `activation`: The activation information for the environment
- `platforms`: The platforms the environment can be run on.
- `channels`: The channels used to create the environment.
- `target`: All the above features but also separated by targets.
- `tasks`: Feature specific tasks, tasks in one environment are selected as default tasks for the environment.


```toml title="Default features" linenums="1"
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

```toml title="Different dependencies per feature" linenums="1"
[feature.py39.dependencies]
python = "~=3.9.0"
[feature.py310.dependencies]
python = "~=3.10.0"
[feature.test.dependencies]
pytest = "*"
```

```toml title="Full set of environment modification in one feature" linenums="1"
[feature.cuda]
dependencies = {cuda = "x.y.z", cudnn = "12.0"}
pypi-dependencies = {torch = "1.9.0"}
platforms = ["linux-64", "osx-arm64"]
activation = {scripts = ["cuda_activation.sh"]}
channels = ["nvidia"] # Would concat instead of overwrite, so the default channels are still used.
tasks = { warmup = "python warmup.py" }
target.osx-arm64 = {dependencies = {mlx = "x.y.z"}}
```


```toml title="Define tasks as defaults of an environment" linenums="1"
[feature.test.tasks]
test = "pytest"

[environments]
test = ["test"]

# `pixi run test` == `pixi run --envrionment test test`
```

The environment definition should contain the following fields:

- `features: Vec<Feature>`: The features that are included in the environment set, which is also the default field in the environments.
- `environments: Vec<Environment>`: The environments that are included in the environment set. When environments is used, the extra features are **on top** of the included environments.
    Environments are used as a locked base, so the features added to an environment are not allowed to change the locked set. This should result in a failure if the locked set is not compatible with the added features.
- `default-features: bool`: Whether the default features should be included in the environment set.

```toml
[environments]
# `default` environment is now the `default` feature plus the py39 feature
default = ["py39"]
# `lint` environment is now the `lint` feature without the `default` feature or environment
lint = {features = ["lint"], default-features = "false"}
# `dev` environment is now the `default` feature plus the `test` feature, which makes the `default` envriroment is solved without the use of the test feature.
dev = {environments = ["default"], features = ["test"]}
```

```toml title="Creating environments from features" linenums="1"
[environments]
# implicit: default = ["default"]
default = ["py39"] # implicit: default = ["py39", "default"]
py310 = ["py310"] # implicit: py310 = ["py310", "default"]
test = ["test"] # implicit: test = ["test", "default"]
test39 = ["test", "py39"] # implicit: test39 = ["test", "py39", "default"]
lint = {features = ["lint"], default-features = "false"} # no implicit default
```

```toml title="Creating environments from environments" linenums="1"
[environments]
prod = ["py39"]
# Takes the `prod` environment and adds the `test` feature to it without modifying the `prod` environment requirements, solve should fail if requirements don't comply with locked set.
test_prod = {environments = ["prod"], features = ["test"]}
```

### Lockfile Structure
Within the `pixi.lock` file, a package may now include an additional `environments` field, specifying the environment to which it belongs.
To avoid duplication the packages `environments` field may contain multiple environments so the lockfile is of minimal size.
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
pixi run python
# Runs python in the `default` environment
```

```shell title="Activating an specific environment"
pixi run -e test pytest
pixi run --environment test pytest
# Runs `pytest` in the `test` environment
```

```shell title="Activating a shell in an environment"
pixi shell -e cuda
pixi shell --environment cuda
# Starts a shell in the `cuda` environment
```
```shell title="Running any command in an environment"
pixi run -e test any_command
# Runs any_command in the `test` environment which doesn't require to be predefined as a task.
```

```shell title="Interactive selection of environments if task is in multiple environments"
# In the scenario where test is a task in multiple environments, interactive selection should be used.
pixi run test
# Which env?
# 1. test
# 2. test39
```

## Important links
- Initial writeup of the proposal: https://gist.github.com/0xbe7a/bbf8a323409be466fe1ad77aa6dd5428
- GitHub project: https://github.com/orgs/prefix-dev/projects/10

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
      tests:
      name: Test ${{ matrix.environment }}
      runs-on: ubuntu-latest
      strategy:
        matrix:
          environment:
            - pl017
            - pl018
            - pl019
            - pl020
            - py39
            - py310
            - py311
            - py312
      steps:
        - uses: actions/checkout@v4
        - uses: prefix-dev/setup-pixi@v0.5.0
          with:
            # already installs the corresponding environment and caches it
            environments: ${{ matrix.environment }}
        - name: Install dependencies
          run: |
            pixi run --env ${{ matrix.environment }} postinstall
            pixi run --env ${{ matrix.environment }} test
    ```
