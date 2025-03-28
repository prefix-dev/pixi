![Starship with Pixi support](../../assets/starship-light.png#only-light)
![Starship with Pixi support](../../assets/starship-dark.png#only-dark)

[Starship](https://starship.rs) is a cross-platform and cross-shell prompt for developers, similar to oh-my-zsh, but with a focus on performance and simplicity.
In [starship/starship #6335](https://github.com/starship/starship/pull/6335), Pixi support is being added.
This pull request has not been merged at the time of writing.
That's why [@pavelzw](https://github.com/pavelzw) created a conda package for his fork in [prefix.dev/yolo-forge](https://prefix.dev/channels/yolo-forge).
The packages are being built in GitHub Actions in the [pavelzw/yolo-forge GitHub repository](https://github.com/pavelzw/yolo-forge) using `rattler-build`.
You can install it using the following command:

```bash
pixi global install -c https://prefix.dev/yolo-forge -c conda-forge starship-fork-pavelzw
```

!!!tip ""
    For information about how to configure and set up starship, see the [official documentation](https://starship.rs).

In order for starship to always find the right python executable, you can adjust its configuration file.

```toml title="~/.config/starship.toml"
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
