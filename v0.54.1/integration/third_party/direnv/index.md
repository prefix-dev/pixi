`direnv` is a tool which automatically activates an environment as soon as you enter a directory with a `.envrc` file that you accepted at some point. This tutorial will demonstrate how to use `direnv` with Pixi\`.

First install `direnv` by running the following command:

```bash
pixi global install direnv

```

Then create a `.envrc` file in your Pixi workspace root with the following content:

.envrc

```shell
watch_file pixi.lock # (1)!
eval "$(pixi shell-hook)" # (2)!

```

1. This ensures that every time your `pixi.lock` changes, `direnv` invokes the shell-hook again.
1. This installs the environment if needed, and activates it. `direnv` ensures that the environment is deactivated when you leave the directory.

```shell
$ cd my-project
direnv: error /my-project/.envrc is blocked. Run `direnv allow` to approve its content
$ direnv allow
direnv: loading /my-project/.envrc
✔ Project in /my-project is ready to use!
direnv: export +CONDA_DEFAULT_ENV +CONDA_PREFIX +PIXI_ENVIRONMENT_NAME +PIXI_ENVIRONMENT_PLATFORMS +PIXI_PROJECT_MANIFEST +PIXI_PROJECT_NAME +PIXI_PROJECT_ROOT +PIXI_PROJECT_VERSION +PIXI_PROMPT ~PATH
$ which python
/my-project/.pixi/envs/default/bin/python
$ cd ..
direnv: unloading
$ which python
python not found

```

While `direnv` comes with [hooks for the common shells](https://direnv.net/docs/hook.html), these hooks into the shell should not be relied on when using and IDE.

Here you can see how to set up `direnv` for your favorite editor:

- [VSCode](../../editor/vscode/#direnv-extension)
- [Jetbrains](../../editor/jetbrains/#direnv)
- [Zed](../../editor/zed/)
