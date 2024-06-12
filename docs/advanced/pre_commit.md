---
part: pixi
title: Pre-commit
description: Using pre-commit with pixi
---

[Pre-commit](https://github.com/pre-commit/pre-commit) is a tool for managing and maintaining tools that run before a commit is made.
It is widely used to ensure that code is formatted correctly, that there are no syntax errors, and that the code is linted.
Pre-commit uses a configuration file called `.pre-commit-config.yaml` to define all pre-commit hooks that should be run.
They usually point to a repository that contains the hook and the version of the hook that should be used.
Since pixi already handles your dependencies, it might make sense for you to instead use pixi in combination with `pixi run` to manage your pre-commit hooks.
For this, you can define a `local` repository and specify the hook you want to use there.

```yaml title=".pre-commit-config.yaml"
exclude: ^.pixi/ # (1)!
repos:
  - repo: local
    hooks:
      - id: pixi-install # (2)!
        name: pixi-install
        entry: pixi install -e lint
        language: system
        always_run: true
        require_serial: true
        pass_filenames: false
      # ruff
      - id: ruff
        name: ruff
        entry: pixi run -e lint ruff check --fix --exit-non-zero-on-fix --force-exclude
        language: system
        types_or: [python, pyi]
        require_serial: true
      - id: ruff-format
        name: ruff-format
        entry: pixi run -e lint ruff format --force-exclude
        language: system
        types_or: [python, pyi]
        require_serial: true
      # nbstripout
      - id: nbstripout
        name: nbstripout
        entry: pixi run -e lint nbstripout
        language: system
        types: [jupyter]
      # pre-commit-hooks
      - id: trailing-whitespace-fixer
        name: trailing-whitespace-fixer
        entry: pixi run -e lint trailing-whitespace-fixer
        language: system
        types: [text]
      - id: end-of-file-fixer
        name: end-of-file-fixer
        entry: pixi run -e lint end-of-file-fixer
        language: system
        types: [text]
```

1. Pre-commit doesn't take `.gitignore` into account when running `pre-commit run -a`.
2. This hook is a workaround for an issue with pixi. It ensures that the pixi environments are up to date before running the other hooks.
   For more information, see [#1482](https://github.com/prefix-dev/pixi/issues/1482).

This has the advantage that you only have one place where you specify your dependency versions instead of having to manage them in multiple places.
Thus, you don't have to worry about keeping the `.pre-commit-config.yaml` up to date with the latest versions of the hooks since `pixi update` can do that for you.
Since you use a `local` repository, you also don't need to rely on downloading the hook definitions from the internet which can be useful in corporate environments.
Also, this doesn't require pre-commit to first install isolated environments for each hook, which can be time-consuming.

!!!tip "Adding new hooks"
    When you want to convert a "classical" pre-commit hook (i.e. defined in some repository) to a pixi hook, you can use the following steps:

    1. Open the URL of the repository of this hook, for example <https://github.com/astral-sh/ruff-pre-commit>.
    2. Take a look at `.pre-commit-hooks.yaml`.
    3. Copy the hook that you want to use and prefix the `entry` with `pixi run -e lint`. Also, change the `language` from `python`, `conda`, etc. to `system` to ensure that pre-commit doesn't try to install an isolated environment for this hook.
    4. Ensure that the dependencies needed (e.g. `ruff`) are specified in your `pixi.toml` in the `lint` feature.

For these hooks to be available, you need to specify them in your `pixi.toml`:

```toml title="pixi.toml"
[feature.lint.dependencies]
pre-commit = "*"
ruff = "*"
nbstripout = "*"
pre-commit-hooks = "*"
[feature.lint.tasks]
pre-commit-install = "pre-commit install"
pre-commit-run = "pre-commit run"

[environments]
lint = { features = ["lint"], no-default-feature = true }
```

You can install the pre-commit hooks by running `pixi run pre-commit-install` and manually run them using `pixi run pre-commit-run`.

## Running pre-commit hooks in CI

As this workflow requires pixi to be installed, you cannot use it in combination with [pre-commit.ci](https://pre-commit.ci).
Instead, you can use the following script to run the pre-commit hooks in your CI:

```yaml title=".github/workflows/ci.yml"
jobs:
  pre-commit-checks:
    name: Pre-commit Checks
    timeout-minutes: 30
    runs-on: ubuntu-latest
    steps:
      - name: Checkout branch
        uses: actions/checkout@v4
      - name: Set up pixi
        uses: prefix-dev/setup-pixi@v0.8.1
        with:
          environments: lint
      - name: pre-commit
        run: pixi run pre-commit-run --color=always --show-diff-on-failure
      - name: Commit changes # (1)!
        uses: pre-commit-ci/lite-action@v1.0.2
        if: always()
```

1. This step is optional and only needed if you want to commit the changes made by the pre-commit hooks back to the repository.
