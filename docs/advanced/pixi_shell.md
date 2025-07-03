The `pixi shell` command is similar to `conda activate` but works a little different under the hood.
Instead of requiring a change to your `~/.bashrc` or other files, it will launch a fresh shell.
That also means that, instead of `conda deactivate`, it's enough to just exit the current shell, e.g. by pressing `Ctrl+D`.

```shell
pixi shell
```

On Unix systems the shell command works by creating a "fake" PTY session that will start the shell, and then send a string like `source /tmp/activation-env-12345.sh` to the `stdin` in order to activate the environment. If you would peek under the hood of the the `shell` command, then you would see that this is the first thing executed in the new shell session.

The temporary script that we generate ends with `echo "PIXI_ENV_ACTIVATED"` which is used to detect if the environment was activated successfully. If we do not receive this string after three seconds, we will issue a warning to the user.

## Issues With Pixi Shell

As explained, `pixi shell` only works well if we execute the activation script _after_ launching shell. Certain commands that are run in the `~/.bashrc` might swallow the activation command, and the environment won't be activated.

For example, if your `~/.bashrc` contains code like the following, `pixi shell` has little chance to succeed:

```shell
# on WSL - the `wsl.exe` somehow takes over `stdin` and prevents `pixi shell` from succeeding
wsl.exe -d wsl-vpnkit --cd /app service wsl-vpnkit start

# on macOS or Linux, some users start fish or nushell from their `bashrc`
# If you wish to start an alternative shell from bash, it's better to do so
# from `~/.bash_profile` or `~/.profile`
if [[ $- = *i* ]]; then
  exec ~/.pixi/bin/fish
fi
```

In order to fix this, we would advise you to follow the steps below to use `pixi shell-hook` instead.

--8<-- "docs/partials/conda-style-activation.md"
