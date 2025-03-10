---8<--- [start:example]

## Examples

```shell
pixi update numpy # (1)!
pixi update numpy pandas # (2)!
pixi update --manifest-path ~/myworkspace/pixi.toml numpy # (3)!
pixi update --environment lint python # (4)!
pixi update -e lint -e schema -e docs pre-commit # (5)!
pixi update --platform osx-arm64 mlx # (6)!
pixi update -p linux-64 -p osx-64 numpy  # (7)!
pixi update --dry-run numpy # (8)!
pixi update --no-install boto3 # (9)!
```

1. This will update the `numpy` package to the latest version that fits the requirement.
2. This will update the `numpy` and `pandas` packages to the latest version that fits the requirement.
3. This will update the `numpy` package to the latest version in the manifest file at the given path.
4. This will update the `python` package in the `lint` environment.
5. This will update the `pre-commit` package in the `lint`, `schema`, and `docs` environments.
6. This will update the `mlx` package in the `osx-arm64` platform.
7. This will update the `numpy` package in the `linux-64` and `osx-64` platforms.
8. This will show the packages that would be updated without actually updating them in the lockfile
9. This will update the `boto3` package in the manifest and lockfile, without installing it in an environment.

--8<-- [end:example]
