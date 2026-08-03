# Git Hooks

Git hooks let you run formatters, linters, and other checks before committing or pushing code.
Since `pixi` manages your environment, you can run those checks within the `pixi` environment, ensuring that everyone on your team uses the same tools and versions without the overhead of duplicate virtual environments.

This guide shows how to use [Lefthook](https://github.com/evilmartians/lefthook), [pre-commit](https://pre-commit.com/), and [prek](https://github.com/j178/prek) with `pixi`.

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


!!! tip "Using pre-commit in CI"
    This approach is **not compatible with [pre-commit.ci](https://pre-commit.ci)**, since `pixi` is not
    pre-installed in that environment.
 
    Instead, run your hooks directly in GitHub Actions using:
 
    ```shell
    pixi run pre-commit run --all-files --show-diff-on-failure
    ```



## prek

[prek](https://github.com/j178/prek) is a faster, Rust-based, drop-in replacement for `pre-commit`.
It uses the **exact same** `.pre-commit-config.yaml` configuration format, so no changes to your
existing hook definitions are needed.
 
To use `prek` with `pixi`, add it to your project:
 
```shell
pixi add prek
```
 
Install the git hooks:
 
```shell
pixi run prek install
```
 
From this point on, `prek` will run your hooks on every commit using your existing
`.pre-commit-config.yaml` — no additional configuration is required.
 
!!! tip
    If you are already using `pre-commit`, switching to `prek` is as simple as replacing
    `pre-commit` with `prek` in your commands. Your existing `.pre-commit-config.yaml`
    works without any modifications.
