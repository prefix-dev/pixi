# Git Hooks

When working with `pixi`, you might want to use Git hooks to ensure that your code is formatted and linted properly before committing or pushing it.
Since `pixi` manages your environment, you can use these tools to run your checks within the `pixi` environment, ensuring that everyone on your team is using the same tools and versions without the overhead of downloading duplicate virtual environments.

We recommend using a Git hook manager to manage your hooks. The most popular ones are [Lefthook](https://github.com/evilmartians/lefthook), [pre-commit](https://pre-commit.com/), and [prek](https://github.com/pavelzw/prek).

## Lefthook

[Lefthook](https://github.com/evilmartians/lefthook) is a fast and powerful Git hooks manager for any type of project.

To use Lefthook with `pixi`, you can add it to your project's dependencies:

```shell
pixi add lefthook
```

Then, initialize and configure `lefthook` by creating a `lefthook.yaml` file in the root of your project:

```yaml title="lefthook.yaml"
# Run lefthook via pixi
lefthook: pixi run --no-progress lefthook
no_auto_install: true

# Use template for `run` so we don't have to repeat the flags
templates:
  run: run --quiet --no-progress --as-is

pre-commit:
  parallel: true
  jobs:
    - name: pixi-install
      run: pixi install
    - group:
        parallel: true
        jobs:
          - name: ruff-check
            glob: "*.{py,pyi}"
            run: pixi {run} ruff check --fix --exit-non-zero-on-fix --force-exclude {staged_files}
          - name: ruff-format
            glob: "*.{py,pyi}"
            run: pixi {run} ruff format --force-exclude {staged_files}
```

Make sure to install the hooks into your Git repository:

```shell
pixi run lefthook install
```

With this configuration, Lefthook will use `pixi run` to execute your hooks, ensuring they run within the correct environment. The `--quiet` and `--no-progress` flags are useful to avoid cluttering the output.

## pre-commit

[pre-commit](https://pre-commit.com/) is a framework for managing and maintaining multi-language pre-commit hooks.

You can add `pre-commit` to your project:

```shell
pixi add pre-commit
```

Create a `.pre-commit-config.yaml` file in the root of your project:

```yaml title=".pre-commit-config.yaml"
repos:
  - repo: local
    hooks:
      - id: ruff-check
        name: ruff-check
        entry: pixi run --quiet --no-progress ruff check --force-exclude
        language: system
        types_or: [python, pyi]
        require_serial: true
      - id: ruff-format
        name: ruff-format
        entry: pixi run --quiet --no-progress ruff format --force-exclude
        language: system
        types_or: [python, pyi]
        require_serial: true
```

Install the `pre-commit` hooks into your repository:

```shell
pixi run pre-commit install
```

By defining the hooks as `local` and `language: system`, `pre-commit` will not try to manage the environments itself, but will instead rely on `pixi run` to execute the commands within the `pixi` environment.

## prek

[prek](https://github.com/pavelzw/prek) is a fast, language-agnostic pre-commit hook manager.

To use `prek`, you can configure it similarly to Lefthook. In your `.prek.toml`:

```toml title=".prek.toml"
[pre-commit]
parallel = true

[[pre-commit.jobs]]
name = "ruff-check"
include = ["*.py", "*.pyi"]
command = "pixi run --quiet --no-progress ruff check {staged_files}"

[[pre-commit.jobs]]
name = "ruff-format"
include = ["*.py", "*.pyi"]
command = "pixi run --quiet --no-progress ruff format {staged_files}"
```

Then, you can run the hooks via `pixi`:

```shell
pixi run prek pre-commit
```
