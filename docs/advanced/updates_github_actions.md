---
part: pixi/advanced
title: Update lockfiles with GitHub Actions
description: Learn how to use GitHub Actions to automatically update your pixi lockfiles.
---

You can leverage GitHub Actions in combination with [pavelzw/pixi-diff-to-markdown](https://github.com/pavelzw/pixi-diff-to-markdown)
to automatically update your lockfiles similar to dependabot or renovate in other ecosystems.

![Update lockfiles](../assets/update-lockfile-light.png#only-light)
![Update lockfiles](../assets/update-lockfile-dark.png#only-dark)

!!!note "Dependabot/Renovate support for pixi"
    You can track native Dependabot support for pixi in [dependabot/dependabot-core #2227](https://github.com/dependabot/dependabot-core/issues/2227#issuecomment-1709069470)
    and for Renovate in [renovatebot/renovate #2213](https://github.com/renovatebot/renovate/issues/2213).

## How to use

To get started, create a new GitHub Actions workflow file in your repository.

```yaml title=".github/workflows/update-lockfiles.yml"
name: Update lockfiles

permissions: # (1)!
  contents: write
  pull-requests: write

on:
  workflow_dispatch:
  schedule:
    - cron: 0 5 1 * * # (2)!

jobs:
  pixi-update:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Set up pixi
        uses: prefix-dev/setup-pixi@v0.8.1
        with:
          run-install: false
      - name: Update lockfiles
        run: |
          set -o pipefail
          pixi update --json | pixi exec pixi-diff-to-markdown >> diff.md
      - name: Create pull request
        uses: peter-evans/create-pull-request@v7
        with:
          token: ${{ secrets.GITHUB_TOKEN }}
          commit-message: Update pixi lockfile
          title: Update pixi lockfile
          body-path: diff.md
          branch: update-pixi
          base: main
          labels: pixi
          delete-branch: true
          add-paths: pixi.lock
```

1. Needed for `peter-evans/create-pull-request`
2. Runs at 05:00, on day 1 of the month

In order for this workflow to work, you need to set "Allow GitHub Actions to create and approve pull requests" to true in your repository settings (in "Actions" -> "General").

!!! tip

    If you don't have any `pypi-dependencies`, you can use `pixi update --json --no-install` to speed up diff generation.

![Allow GitHub Actions PRs](../assets/allow-github-actions-prs-light.png#only-light)
![Allow GitHub Actions PRs](../assets/allow-github-actions-prs-dark.png#only-dark)

## Triggering CI in automated PRs

In order to prevent accidental recursive GitHub Workflow runs, GitHub decided to not trigger any workflows on automated PRs when using the default `GITHUB_TOKEN`.
There are a couple of ways how to work around this limitation. You can find excellent documentation for this in `peter-evans/create-pull-request`, see [here](https://github.com/peter-evans/create-pull-request/blob/main/docs/concepts-guidelines.md#triggering-further-workflow-runs).

## Customizing the summary

You can customize the summary by either using command-line-arguments of `pixi-diff-to-markdown` or by specifying the configuration in `pixi.toml` under `[tool.pixi-diff-to-markdown]`. See the [pixi-diff-to-markdown documentation](https://github.com/pavelzw/pixi-diff-to-markdown) or run `pixi-diff-to-markdown --help` for more information.

## Using reusable workflows

If you want to use the same workflow in multiple repositories in your GitHub organization, you can create a reusable workflow.
You can find more information in the [GitHub documentation](https://docs.github.com/en/actions/using-workflows/reusing-workflows).
