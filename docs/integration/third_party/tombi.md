[Tombi](https://tombi-toml.github.io/tombi) is a TOML toolkit with a formatter, linter and language server.
It already provides schema-based validation and completion for `pixi.toml` out of the box.

There is an unofficial [fork of tombi](https://github.com/pavelzw/tombi/pull/1) that adds a pixi extension, similar to tombi's Cargo extension: it shows the available versions of conda packages in your `pixi.toml` dependencies.
This is what it looks like in [diffle](https://diffle.app):

![Tombi with pixi extension](../../assets/tombi-light.png#only-light)
![Tombi with pixi extension](../../assets/tombi-dark.png#only-dark)

You can install it using `pixi global`:

```bash
pixi global install -c https://prefix.dev/yolo-forge -c conda-forge tombi-pavelzw
```

!!! warning
    This fork is not maintained by the tombi project.
    It will be updated to newer tombi releases on a best-effort basis.
    See [tombi-toml/tombi#2337](https://github.com/tombi-toml/tombi/issues/2337#issuecomment-6312325741) for more details.

`pixi global` puts `tombi` into `~/.pixi/bin`, so it is available on your `PATH`.
Depending on your editor, you might need to configure it to use this `tombi` instead of a bundled one.

### VS Code

The [Tombi extension](https://marketplace.visualstudio.com/items?itemName=tombi-toml.tombi) prefers `tombi` from a virtual environment or `node_modules` in your workspace over the one on your `PATH`.
To always use the fork, set its path in your user settings:

```json title="settings.json"
{
  "tombi.path": "/home/<user>/.pixi/bin/tombi"
}
```

### Zed

Point the [Tombi extension](https://zed.dev/extensions/tombi) to the fork in your Zed settings:

```json title="~/.config/zed/settings.json"
{
  "lsp": {
    "tombi": {
      "binary": {
        "path": "/home/<user>/.pixi/bin/tombi",
        "arguments": ["lsp"]
      }
    }
  }
}
```

### diffle

[diffle](https://diffle.app) uses `tombi lsp` for TOML files whenever `tombi` is on your `PATH`, so no configuration is needed.
You can check which language server it picks up with `diffle lsp`.
