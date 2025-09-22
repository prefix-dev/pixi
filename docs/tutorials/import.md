In this tutorial we will show you how to import existing environments into a Pixi workspace.
In case some words used in the tutorial don't make sense to you, you may get value from first
reading some of our other tutorials, like [our first workspace walthrough](../first_workspace.md) and [our guide to multi-environment workspaces](./multi_environment.md).

## `pixi import`
Within any Pixi workspace, you can use [`pixi import`](https://pixi.sh/latest/reference/cli/pixi/import/) to import an environment from a given file. At the time of writing, we support two import file formats: `conda-env` and `pypi-txt`. Running `pixi import` without providing a `format` will try each format in turn until one succeeds, or return an error if all formats fail.

If you don't already have a Pixi workspace, you can create one with [`pixi init`](https://pixi.sh/latest/reference/cli/pixi/init/).

### `conda-env` format
The `conda-env` format is for files in the conda ecosystem (typically called `environment.yml`) following [the syntax specified in the conda docs](https://docs.conda.io/projects/conda/en/latest/user-guide/tasks/manage-environments.html#create-env-file-manually). Suppose our environment to import is specified in this file:

```yaml title="environment.yml"
name: simple-env
channels: ["conda-forge"]
dependencies:
- python
```

We can then run `pixi import --format=conda-env environment.yml` to import the environment into our workspace. By default, since our `environment.yml` has a `name` field, this creates a `feature` of the same name (or uses the feature of that name if it already exists), and creates an `environment` containing that feature (with [`no-default-feature`](https://pixi.sh/latest/reference/pixi_manifest/#the-environments-table) set):

```toml title="pixi.toml"
[feature.simple-env]
channels = ["conda-forge"]

[feature.simple-env.dependencies]
python = "*"

[environments]
simple-env = { features = ["simple-env"], no-default-feature = true }
```

For files without a `name` field, or to override the default behaviour, you can specify custom `--feature` and `--environment` names. This also allows importing into existing features and environments (including the `default` feature and environment). For example, given this other environment file to import:

```yaml title="env2.yml"
channels: ["conda-forge"]
dependencies:
- numpy
```

Running `pixi import --format=conda-env --feature=numpy --environment=simple-env env2.yml` will import the environment into a new feature called "numpy", and include that feature in the existing `simple-env` environment (effectively merging the environments from our two input files):

```toml title="pixi.toml"
[feature.simple-env]
channels = ["conda-forge"]

[feature.simple-env.dependencies]
python = "*"

[feature.numpy]
channels = ["conda-forge"]

[feature.numpy.dependencies]
numpy = "*"

[environments]
simple-env = { features = ["simple-env", "numpy"], no-default-feature = true }
```

It is also possible to specify platforms for the feature via the `--platform` argument. For example, `pixi import --format=conda-env --feature=unix --platform=linux-64 --platform=osx-arm64 environment.yml` adds the following to our workspace manifest:

```toml title="pixi.toml"
[feature.unix]
platforms = ["linux-64", "osx-arm64"]
channels = ["conda-forge"]

[feature.unix.target.linux-64.dependencies]
python = "*"

[feature.unix.target.osx-arm64.dependencies]
python = "*"

[environments]
unix = { features = ["unix"], no-default-feature = true }
```

### `pypi-txt` format
The `pypi-txt` format is for files in the PyPI ecosystem following [the requirements file format specification in the `pip` docs](https://pip.pypa.io/en/stable/reference/requirements-file-format/).

Suppose our environment to import is specified in this file:

```yaml title="requirements.txt"
cowpy
array-api-extra
```

We can then run `pixi import --format=pypi-txt --feature=my-feature1 requirements.txt` to import the environment into our workspace. It is necessary to specify a `feature` or `environment` name (or both) via the arguments of the same names. If only one of these names is provided, a matching name is used for the other field. Hence, the following lines are added to our workspace manifest:

```toml title="pixi.toml"
[feature.my-feature1.pypi-dependencies]
cowpy = "*"
array-api-extra = "*"

[environments]
my-feature1 = { features = ["my-feature1"], no-default-feature = true }
```

Any dependencies listed in the file are added as [`pypi-dependencies`](https://pixi.sh/latest/reference/pixi_manifest/#pypi-dependencies). An environment will be created with [`no-default-feature`](https://pixi.sh/latest/reference/pixi_manifest/#the-environments-table) set if the given environment name does not already exist.

Just like the `conda-env` format, it is possible to import into existing features/environments (including the `default` feature/environment), and set specific platforms for the feature. See the previous section for details.

## `pixi init --import`
It is also possible to combine the steps of `pixi init` and `pixi import` into one, via [`pixi init --import`](https://pixi.sh/latest/reference/cli/pixi/init/#arg---import). For example, `pixi init --import environment.yml` (using the same file from our example above) produces a manifest which looks like this:

```toml title="pixi.toml"
[workspace]
authors = ["Lucas Colley <lucas.colley8@gmail.com>"]
channels = ["conda-forge"]
name = "simple-env"
platforms = ["osx-arm64"]
version = "0.1.0"

[tasks]

[dependencies]
python = "*"
```

Unlike `pixi import`, this by default uses the `default` feature and environment. Thus, it achieves a very similar workspace to that obtained by running `pixi init ` and `pixi import --feature=default environment.yml`.

One difference is that `pixi init --import` will by default inherit its name from the given import file (if the file specifies the `name` field), rather than from its working directory.

??? warning "Supported formats"
    At the time of writing, only the `conda-env` format is supported by `pixi init --import`.

## Conclusion
For further details, please see the CLI reference documentation for [`pixi import`](https://pixi.sh/latest/reference/cli/pixi/import/) and [`pixi init --import`](https://pixi.sh/latest/reference/cli/pixi/init/#arg---import).
If there are any questions, or you know how to improve this tutorial, feel free to reach out to us on [GitHub](https://github.com/prefix-dev/pixi).

At the time of writing, there are plans for many potential extensions to our import capabilities — you can follow along with that work at [the `import` roadmap issue on GitHub](https://github.com/prefix-dev/pixi/issues/4192).
