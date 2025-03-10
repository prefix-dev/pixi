---8<--- [start:example]

## Examples

```shell
pixi install  # (1)!
pixi install --manifest-path ~/myworkspace/pixi.toml # (2)!
pixi install --frozen # (3)!
pixi install --locked # (4)!
pixi install --environment lint # (5)!
pixi install -e lint # (5)!
```

1. This will install the default environment.
2. This will install the default environment from the manifest file at the given path.
3. This will install the environment from the lockfile without updating the lockfile.
4. This will install the environment from the lockfile without updating the lockfile and ensuring the environment is locked correctly.
5. This will install the `lint` environment.

--8<-- [end:example]
