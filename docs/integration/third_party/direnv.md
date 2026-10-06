
`direnv` is a tool which automatically activates an environment as soon as you enter a directory with a `.envrc` file that you accepted at some point.
This tutorial will demonstrate how to use `direnv` with Pixi.

First install `direnv` by running the following command:

```bash
pixi global install direnv
```

Then create a `.envrc` file in your Pixi workspace root with the following content:

```shell title=".envrc"
require_allowed pixi.toml # (1)!
layout pixi # (2)!
```

1. Requires a fresh `direnv allow` whenever `pixi.toml` changes. Use `pyproject.toml` instead if your workspace is configured there.
2. Installs the environment if needed and activates it by running `pixi shell-hook`. It also requires a fresh `direnv allow` whenever `pixi.lock` changes. `direnv` ensures that the environment is deactivated when you leave the directory.

`layout pixi` passes its arguments to `pixi shell-hook`, so you can activate a different environment with `layout pixi -e <env_name>`.

!!! warning "Require approval for manifest and lock file changes"
    Activating a Pixi environment can execute arbitrary code, for example through [activation scripts](../../workspace/environment.md#activation) of installed packages.
    `require_allowed` makes sure that `direnv` does not activate an environment from a changed manifest or lock file (e.g. after `git pull` or switching branches) until you have reviewed the change and run `direnv allow` again.
    See [Supply Chain Security](../../security.md#4-treat-package-hooks-as-code-execution) for more details.

!!! note "Older `direnv` versions"
    `layout pixi` and `require_allowed` are available since `direnv` v2.38.1.
    On older versions you can use the following `.envrc` instead, but be aware that it re-activates the environment after changes to `pixi.lock` without asking for approval:

    ```shell title=".envrc"
    watch_file pixi.lock
    eval "$(pixi shell-hook)"
    ```

```shell
$ cd my-project
direnv: error /my-project/.envrc is blocked. Run `direnv allow` to approve its content
$ direnv allow
direnv: allowing pixi.toml
direnv: allowing pixi.lock
direnv: loading /my-project/.envrc
✔ Project in /my-project is ready to use!
direnv: export +CONDA_DEFAULT_ENV +CONDA_PREFIX +PIXI_ENVIRONMENT_NAME +PIXI_ENVIRONMENT_PLATFORMS +PIXI_WORKSPACE_MANIFEST +PIXI_WORKSPACE_NAME +PIXI_WORKSPACE_ROOT +PIXI_WORKSPACE_VERSION +PIXI_PROMPT ~PATH
$ which python
/my-project/.pixi/envs/default/bin/python
$ cd ..
direnv: unloading
$ which python
python not found
```

While `direnv` comes with [hooks for the common shells](https://direnv.net/docs/hook.html),
these hooks into the shell should not be relied on when using and IDE.

Here you can see how to set up `direnv` for your favorite editor:

- [VSCode](../editor/vscode.md#direnv-extension)
- [Jetbrains](../editor/jetbrains.md#direnv)
- [Zed](../editor/zed.md)
