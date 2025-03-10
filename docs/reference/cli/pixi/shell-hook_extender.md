--8<-- [start:example]

## Examples

```shell
pixi shell-hook
pixi shell-hook --shell bash
pixi shell-hook --shell zsh
pixi shell-hook -s powershell
pixi shell-hook --manifest-path ~/myworkspace/pixi.toml
pixi shell-hook --frozen
pixi shell-hook --locked
pixi shell-hook --environment cuda
pixi shell-hook --json
```

Example use-case, when you want to get rid of the `pixi` executable in a Docker container.

```shell
pixi shell-hook --shell bash > /etc/profile.d/pixi.sh
rm ~/.pixi/bin/pixi # Now the environment will be activated without the need for the pixi executable.
```

--8<-- [end:example]
