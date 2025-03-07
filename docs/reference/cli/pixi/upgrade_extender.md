---8<--- [start:example]

## Examples

```shell
pixi upgrade # (1)!
pixi upgrade numpy # (2)!
pixi upgrade numpy pandas # (3)!
pixi upgrade --manifest-path ~/myproject/pixi.toml numpy # (4)!
pixi upgrade --feature lint python # (5)!
pixi upgrade --json # (6)!
pixi upgrade --dry-run # (7)!
```

1. This will upgrade all packages to the latest version.
2. This will upgrade the `numpy` package to the latest version.
3. This will upgrade the `numpy` and `pandas` packages to the latest version.
4. This will upgrade the `numpy` package to the latest version in the manifest file at the given path.
5. This will upgrade the `python` package in the `lint` feature.
6. This will upgrade all packages and output the result in JSON format.
7. This will show the packages that would be upgraded without actually upgrading them in the lockfile or manifest.

--8<-- [end:example]
