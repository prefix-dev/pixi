[Tombi](https://tombi-toml.github.io/tombi) is a TOML toolkit with a formatter, linter and language server.
It already provides schema-based validation and completion for `pixi.toml` out of the box.

There is an unofficial [fork of tombi](https://github.com/pavelzw/tombi/pull/1) that adds a pixi extension, similar to tombi's Cargo extension: it shows the available versions of conda packages in your `pixi.toml` dependencies, fetched from conda-forge using [sharded repodata](https://conda.org/learn/ceps/cep-0016).
It is intended as a stopgap until tombi supports third-party plugins (see [tombi-toml/tombi#1305](https://github.com/tombi-toml/tombi/issues/1305)).

You can install it using `pixi global`:

```bash
pixi global install -c https://prefix.dev/yolo-forge -c conda-forge tombi-pavelzw
```

!!! warning
    This fork is not maintained by the tombi project.
    It will be updated to newer tombi releases on a best-effort basis.
    See [tombi-toml/tombi#2337](https://github.com/tombi-toml/tombi/issues/2337#issuecomment-6312325741) for more details.
