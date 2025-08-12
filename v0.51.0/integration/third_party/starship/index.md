[Starship](https://starship.rs) is a cross-platform and cross-shell prompt for developers, similar to oh-my-zsh, but with a focus on performance and simplicity. It also has full Pixi support. You can install it using the following command:

```bash
pixi global install starship

```

For information about how to configure and set up starship, see the [official documentation](https://starship.rs/config/#pixi).

In order for starship to always find the right python executable, you can adjust its configuration file.

~/.config/starship.toml

```toml
[python]
# customize python binary path for pixi
python_binary = [
  # this is the python from PATH if in a pixi shell
  # (assuming you don't have python on your global PATH)
  "python",
  # fall back to pixi's python if it's available
  ".pixi/envs/default/bin/python",
]

```

By default, starship uses 🧚🏻 as pixi's symbol. You can adjust it as follows if you want a different symbol

~/.config/starship.toml

```toml
[pixi]
symbol = "📦 "

```

As starship already displays a custom message when a pixi environment is active, you can disable pixi's custom PS1:

```text
pixi config set shell.change-ps1 "false"

```
