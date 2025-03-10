--8<-- [start:example]

## Examples

```shell
pixi project export conda-environment environment.yml
```

The [`environment.yml` file](https://conda.io/projects/conda/en/latest/user-guide/tasks/manage-environments.html#creating-an-environment-from-an-environment-yml-file) can then be used to create a conda environment using conda/mamba:

```shell
mamba create --name <env> --file environment.yml
```
--8<-- [end:example]
