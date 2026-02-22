[pixi-inject](https://github.com/pavelzw/pixi-inject) is a simple executable that injects a conda package into an existing pixi environment.

```
pixi inject --environment default --package my-package-0.1.0-py313h8aa417a_0.conda
```

You can also specify a custom conda prefix to inject the package into.

```
pixi inject --prefix /path/to/conda/env --package my-package-0.1.0-py313h8aa417a_0.conda
```
