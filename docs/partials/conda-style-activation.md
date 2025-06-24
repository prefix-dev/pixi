## Traditional `conda activate`-like activation

If you prefer to use the traditional `conda activate`-like activation, you could use the `pixi shell-hook` command.

```shell
$ which python
python not found
$ eval "$(pixi shell-hook)"
$ (default) which python
/path/to/project/.pixi/envs/default/bin/python
```

For example, with `bash` and `zsh` you can use the following command:

```shell
eval "$(pixi shell-hook)"
```

??? tip  "Custom activation function"
    With the `--manifest-path` option you can also specify which environment to activate. If you want to add a `bash` function to your `~/.bashrc` that will activate the environment, you can use the following command:

    === "Bash/Zsh"
        ```shell
        function pixi_activate() {
            # default to current directory if no path is given
            local manifest_path="${1:-.}"
            eval "$(pixi shell-hook --manifest-path $manifest_path)"
        }
        ```

        After adding this function to your `~/.bashrc`/`~/.zshrc`, you can activate the environment by running:


    === "Fish"

        With fish, you can also evaluate the output of `pixi shell-hook`:

        ```fish
        pixi shell-hook | source
        ```

        Or, if you want to add a function to your `~/.config/fish/config.fish`:

        ```fish
        function pixi_activate
            # default to current directory if no path is given
            set -l manifest_path $argv[1]
            test -z "$manifest_path"; and set manifest_path "."

            pixi shell-hook --manifest-path "$manifest_path" | source
        end
        ```
        After adding this function to your `~/.config/fish/config.fish`, you can activate the environment by running:

    ```shell
    pixi_activate

    # or with a specific manifest
    pixi_activate ~/projects/my_project
    ```



??? tip "Using direnv"
    See our [direnv page](../integration/third_party/direnv.md) on how to leverage `pixi shell-hook` to integrate with direnv.
