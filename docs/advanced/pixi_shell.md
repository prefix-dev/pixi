# pixi shell

The `pixi shell` command is similar to `conda activate` but works a little different under the hood.
Instead of requiring a change to your `~/.bashrc` or other files, it will launch a fresh shell.
That also means that, instead of `conda deactivate`, it's enough to just exit the current shell, e.g. by pressing `Ctrl+D`.

```shell
pixi shell
```

On Unix systems the shell command works by creating a "fake" PTY session that will start the shell, and then send a string like `source /tmp/activation-env-12345.sh` to the `stdin` in order to activate the environment. If you would peek under the hood of the the `shell` command, then you would see that this is the first thing executed in the new shell session.

The temporary script that we generate ends with `echo "PIXI_ENV_ACTIVATED"` which is used to detect if the environment was activated successfully. If we do not receive this string after one second, we will issue a warning to the user.

## Issues with pixi shell

As explained, `pixi shell` only works well if we execute the activation script _after_ launching shell. Certain commands that are run in the `~/.bashrc` might swallow the activation command, and the environment won't be activated.

For example, if your `~/.bashrc` contains code like the following, `pixi shell` has little chance to succeed:

```shell
# on WSL - the `wsl.exe` somehow takes over `stdin` and prevents `pixi shell` from succeeding
wsl.exe -d wsl-vpnkit --cd /app service wsl-vpnkit start

# on macOS or Linux, some users start fish or nushell from their `bashrc`
if [[ $- = *i* ]]; then
  exec ~/.pixi/bin/fish
fi
```

In order to fix this, we would advise you to follow the steps below to use `pixi shell-hook` instead.

## Emulating `conda activate` with pixi

To emulate `conda activate` - which activates a conda environment in the current shell - you can use the `pixi shell-hook` subcommand. The `shell-hook` is going to print a shell script to your `stdout` that can be used by your shell to activate the environment.

For example, with `bash` and `zsh` you can use the following command:

```shell
eval "$(pixi shell-hook)"
```

With the `--manifest-path` option you can also specify which environment to activate. If you want to add a `bash` function to your `~/.bashrc` that will activate the environment, you can use the following command:

```shell
function pixi_activate() {
    # default to current directory if no path is given
    local manifest_path="${1:-.}"
    eval "$(pixi shell-hook --manifest-path '$manifest_path')"
}
```

After adding this function to your `~/.bashrc`, you can activate the environment by running:

```shell
pixi_activate

# or with a specific manifest
pixi_activate ~/projects/my_project
```

### For `fish` users

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
