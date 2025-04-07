
??? note "Installing direnv"

    Of course you can use `pixi` to install `direnv` globally. We recommend to run

    ```bash
    pixi global install direnv
    ```

    to install the latest version of `direnv` on your computer.

You can use `pixi` in combination with `direnv` to automatically activate environments on entering the corresponding directory.
Enter the following into your `.envrc` file:

```shell title=".envrc"
watch_file pixi.lock # (1)!
eval "$(pixi shell-hook)" # (2)!
```

1. This ensures that every time your `pixi.lock` changes, `direnv` invokes the shell-hook again.
2. This installs if needed, and activates the environment. `direnv` ensures that the environment is deactivated when you leave the directory.

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

While `direnv` comes with [hooks for the common shells](https://direnv.net/docs/hook.html), 
these hooks into the shell should not be relied on when using and IDE.
Although `direnv` package does not include hooks for integrated develop environments,
there is typically a plugin or extension available.

Here you can see how to set up `direnv` for some of the more popular IDEs:
- [VSCode](../editor/vscode.md): [mkhl.direnv plugin](https://marketplace.visualstudio.com/items?itemName=mkhl.direnv)
- [Jetbrains](../editor/jetbrains.md): [Direnv Integration plugin](https://plugins.jetbrains.com/plugin/15285-direnv-integration)
- [Zed](../editor/zed.md): [Native support](https://zed.dev/docs/configuring-zed#direnv-integration)
- [Neovim](../editor/neovim.md): [Direnv plugin](https://github.com/direnv/direnv.vim/blob/master/README.md)

