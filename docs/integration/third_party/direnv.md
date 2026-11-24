
`direnv` is a tool which automatically activates an environment as soon as you enter a directory with a `.envrc` file that you accepted at some point.
This tutorial will demonstrate how to use `direnv` with Pixi.

First install `direnv` by running the following command:

```bash
pixi global install direnv
```

Then create a `.envrc` file in your Pixi workspace root with the following content:

```shell title=".envrc"
layout pixi
```

[`layout pixi`](https://direnv.net/man/direnv-stdlib.1.html#codelayout-pixicode) installs the environment if needed and activates it by running `pixi shell-hook`.
`direnv` ensures that the environment is deactivated when you leave the directory.
Additional arguments are passed to `pixi shell-hook`, so you can activate a different environment with `layout pixi -e <env_name>`.

`layout pixi` guards the manifest (`pixi.toml` or `pyproject.toml`) and `pixi.lock` with:

- [`require_allowed`](https://direnv.net/man/direnv-stdlib.1.html#coderequireallowed-ltpathgt-ltpathgt-code), which blocks the `.envrc` until you run `direnv allow` again whenever one of these files changes;
- [`watch_file`](https://direnv.net/man/direnv-stdlib.1.html#codewatchfile-ltpathgt-ltpathgt-code), which makes `direnv` reload the `.envrc` whenever one of these files changes.

!!! warning "Review manifest and lock file changes before running `direnv allow`"
    Activating a Pixi environment can execute arbitrary code, for example through [activation scripts](../../workspace/environment.md#activation) of installed packages or the `[activation]` section of your manifest.
    `require_allowed` makes sure that `direnv` does not activate an environment from a changed manifest or lock file (e.g. after `git pull` or switching branches) until you have reviewed the change.
    If your manifest references activation scripts from your repository, add `require_allowed path/to/script.sh` to your `.envrc` to guard them as well.
    See [Supply Chain Security](../../security.md#4-treat-package-hooks-as-code-execution) for more details.

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
