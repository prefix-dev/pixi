In this tutorial we will show you how to use multiple environments in one Pixi workspace.

## Why Is This Useful?

When developing a workspace you often need different tools, libraries or test environments.
With Pixi you can define multiple environments in one workspace and switch between them easily.
A developer often needs all the tools they can get, whereas your testing infrastructure might not require all those tools, and your production environment might require even less.
Setting up different environments for these different use cases can be a hassle, but with Pixi it's easy.

## Glossary
This tutorial possibly uses some new terms, here is a quick overview:

#### **Feature**
A feature defines a part of an environment, but are not useful without being part of an environment.
You can define multiple features in one workspace.
A feature can contain `tasks`, `dependencies`, `platforms`, `channels` and [more](../reference/pixi_manifest.md#the-feature-table).
You can mix multiple features to create an environment.
Features are defined by adding `[feature.<name>.*]` to a table in the manifest file.
#### **Environment**
An environment is a collection of features.
Environments can actually be installed and activated to run tasks in.
You can define multiple environments in one workspace.
Defining environments is done by adding them to the `[environments]` table in the manifest file.
#### **Default**
Instead of specifying `[feature.<name>.dependencies]`, one can populate `[dependencies]` directly.
These top level table, are added to the "default" feature, which is added to every environment, unless you specifically opt-out.

## Let's Get Started

We'll simply start with a new workspace, you can skip this step if you already have a Pixi workspace.

```shell
pixi init workspace
cd workspace
pixi add python
```

Now we have a new Pixi workspace with the following structure:
```
├── .pixi
│   └── envs
│       └── default
├── pixi.lock
└── pixi.toml
```

Note the `.pixi/envs/default` directory, this is where the default environment is stored.
If no environment is specified, Pixi will create or use the `default` environment.


### Adding a feature
Let's start adding a simple `test` feature to our workspace.
We can do this through the command line, or by editing the `pixi.toml` file.
Here we will use the command line, and add a `pytest` dependency to the `test` feature in our workspace.
```shell
pixi add --feature test pytest
```
This will add the following to our `pixi.toml` file:
```toml
--8<-- "docs/source_files/pixi_tomls/multi-environment-simple.toml:test-feat-dep"
```
This table acts exactly the same as a normal `dependencies` table, but it is only used when the `test` feature is part of an environment.

### Adding an environment
We will add the `test` environment to our workspace to add some testing tools.
We can do this through the command line, or by editing the `pixi.toml` file.
Here we will use the command line:
```shell
pixi workspace environment add test --feature test
```
This will add the following to our `pixi.toml` file:
```toml
--8<-- "docs/source_files/pixi_tomls/multi-environment-simple.toml:test-env"
```

### Running a task
We can now run a task in our new environment.
```shell
pixi run --environment test pytest --version
```
This has created the test environment, and run the `pytest --version` command in it.
You can see the environment will be added to the `.pixi/envs` directory.
```shell
├── .pixi
│   └── envs
│       ├── default
│       └── test
```
If you want to see the environment, you can use the `pixi list` command.
```shell
pixi list --environment test
```

If you have special test commands that always fit with the test environment you can add them to the `test` feature.
```shell
# Adding the 'test' task to the 'test' feature and setting it to run `pytest`
pixi task add test --feature test pytest
```
This will add the following to our `pixi.toml` file:
```toml
--8<-- "docs/source_files/pixi_tomls/multi-environment-simple.toml:test-tasks"
```
Now you don't have to specify the environment when running the test command.
```shell
pixi run test
```
In this example similar to running `pixi run --environment test pytest`

This works as long as there is only one of the environments that has the `test` task.

## Using multiple environments to test multiple versions of a package
In this example we will use multiple environments to test a package against multiple versions of Python.
This is a common use-case when developing a python library.
This workflow can be translated to any setup where you want to have multiple environments to test against a different dependency setups.

For this example we assume you have run the commands in the previous example, and have a project with a `test` environment.
To allow python being flexible in the new environments we need to set it to a more flexible version e.g. `*`.


```shell
pixi add "python=*"
```

We will start by setting up two features, `py311` and `py312`.
```shell
pixi add --feature py311 python=3.11
pixi add --feature py312 python=3.12
```

We'll add the `test` and Python features to the corresponding environments.
```shell
pixi workspace environment add test-py311 --feature py311 --feature test
pixi workspace environment add test-py312 --feature py312 --feature test
```

This should result in adding the following to the `pixi.toml`:
```toml
--8<-- "docs/source_files/pixi_tomls/multi-environment-py-envs.toml:py-envs"
```

Now we can run the test command in both environments.
```shell
pixi run --environment test-py311 test
pixi run --environment test-py312 test
# Or using the task directly, which will spawn a dialog to select the environment of choice
pixi run test
```

These could now run in CI to test separate environments:
```yaml title=".github/workflows/test.yml"
test:
  runs-on: ubuntu-latest
  strategy:
    matrix:
      environment: [test-py311, test-py312]
  steps:
  - uses: actions/checkout@v4
  - uses: prefix-dev/setup-pixi@v0
    with:
      environments: ${{ matrix.environment }}
  - run: pixi run -e ${{ matrix.environment }} test
```
More info on that in the GitHub actions [documentation](../integration/ci/github_actions.md).

## Development, Testing, Production environments
This assumes a clean project, so if you have been following along, you might want to start a new project.
```shell
pixi init production_project
cd production_project
```

Like before we'll start with creating multiple features.
```shell
pixi add numpy python # default feature
pixi add --feature dev jupyterlab
pixi add --feature test pytest
```

Now we'll add the environments.
To accommodate the different use-cases we'll add a `production`, `test` and `default` environment.

- The `production` environment will only have the `default` feature, as that is the bare minimum for the project to run.
- The `test` environment will have the `test` and the `default` features, as we want to test the project and require the testing tools.
- The `default` environment will have the `dev` and `test` features.

We make this the default environment as it will be the easiest to run locally, as it avoids the need to specify the environment when running tasks.

We'll also add the `solve-group` `prod` to the environments, this will make sure that the dependencies are solved as if they were in the same environment.
This will result in the `production` environment having the exact same versions of the dependencies as the `default` and `test` environment.
This way we can be sure that the project will run in the same way in all environments.

```shell
pixi workspace environment add production --solve-group prod
pixi workspace environment add test --feature test --solve-group prod
# --force is used to overwrite the default environment
pixi workspace environment add default --feature dev --feature test --solve-group prod --force
```

If we run `pixi list -x` for the environments we can see that the different environments have the exact same dependency versions.
```shell
# Default environment
Package     Version  Build               Size       Kind   Source
jupyterlab  4.3.4    pyhd8ed1ab_0        6.9 MiB    conda  jupyterlab
numpy       2.2.1    py313ha4a2180_0     6.2 MiB    conda  numpy
pytest      8.3.4    pyhd8ed1ab_1        253.1 KiB  conda  pytest
python      3.13.1   h4f43103_105_cp313  12.3 MiB   conda  python

Environment: test
Package  Version  Build               Size       Kind   Source
numpy    2.2.1    py313ha4a2180_0     6.2 MiB    conda  numpy
pytest   8.3.4    pyhd8ed1ab_1        253.1 KiB  conda  pytest
python   3.13.1   h4f43103_105_cp313  12.3 MiB   conda  python

Environment: production
Package  Version  Build               Size      Kind   Source
numpy    2.2.1    py313ha4a2180_0     6.2 MiB   conda  numpy
python   3.13.1   h4f43103_105_cp313  12.3 MiB  conda  python
```

### Non default environments
When you want to have an environment that doesn't have the `default` feature, you can use the `--no-default-feature` flag.
This will result in the environment not having the `default` feature, and only the features you specify.

A common use-case of this would be having an environment that can generate your documentation.

Let's add the `mkdocs` dependency to the `docs` feature.
```shell
pixi add --feature docs mkdocs
```

Now we can add the `docs` environment without the `default` feature.
```shell
pixi workspace environment add docs --feature docs --no-default-feature
```

If we run `pixi list -x -e docs` we can see that it only has the `mkdocs` dependency.
```shell
Environment: docs
Package  Version  Build         Size     Kind   Source
mkdocs   1.6.1    pyhd8ed1ab_1  3.4 MiB  conda  mkdocs
```

## Conclusion
The multiple environment feature is extremely powerful and can be used in many different ways.
There is much more to explore in the [reference](../reference/pixi_manifest.md#the-feature-and-environments-tables) and [advanced](../workspace/multi_environment.md) sections.
If there are any questions, or you know how to improve this tutorial, feel free to reach out to us on [GitHub](https://github.com/prefix-dev/pixi).
