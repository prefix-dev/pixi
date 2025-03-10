<!--- This file is autogenerated. Do not edit manually! -->
# <code>[pixi](../../../pixi.md) [project](../../project.md) [export](../export.md) conda-environment</code>

## About
Export project environment to a conda environment.yaml file

--8<-- "docs/reference/cli/pixi/project/export/conda-environment_extender.md:description"

## Usage
```
pixi project export conda-environment [OPTIONS] [OUTPUT_PATH]
```

## Arguments
- <a id="arg-<OUTPUT_PATH>" href="#arg-<OUTPUT_PATH>">`<OUTPUT_PATH>`</a>
:  Explicit path to export the environment file to

## Options
- <a id="arg---environment" href="#arg---environment">`--environment (-e) <ENVIRONMENT>`</a>
:  The environment to render the environment file for. Defaults to the default environment
- <a id="arg---platform" href="#arg---platform">`--platform (-p) <PLATFORM>`</a>
:  The platform to render the environment file for. Defaults to the current platform

## Global Options
- <a id="arg---manifest-path" href="#arg---manifest-path">`--manifest-path <MANIFEST_PATH>`</a>
:  The path to `pixi.toml`, `pyproject.toml`, or the project directory

--8<-- "docs/reference/cli/pixi/project/export/conda-environment_extender.md:example"
