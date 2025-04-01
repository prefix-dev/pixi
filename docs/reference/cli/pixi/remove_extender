--8<-- [start:example]
## Examples

```shell
pixi remove numpy
pixi remove numpy pandas pytorch
pixi remove --manifest-path ~/myworkspace/pixi.toml numpy
pixi remove --host python
pixi remove --build cmake
pixi remove --pypi requests
pixi remove --platform osx-64 --build clang
pixi remove --feature featurex clang
pixi remove --feature featurex --platform osx-64 clang
pixi remove --feature featurex --platform osx-64 --build clang
pixi remove --no-install numpy
```

--8<-- [end:example]

--8<-- [start:description]
If the project manifest is a `pyproject.toml`, removing a pypi dependency with the `--pypi` flag will remove it from either

- the native pyproject `project.dependencies` array or the native `project.optional-dependencies` table (if a feature is specified)
- pixi `pypi-dependencies` tables of the default or a named feature (if a feature is specified)
--8<-- [end:description]
