--8<-- [start:example]

## Examples

```shell
pixi project export conda-explicit-spec output
pixi project export conda-explicit-spec -e default -e test -p linux-64 output
```

The [explicit specification file](https://conda.io/projects/conda/en/latest/user-guide/tasks/manage-environments.html#building-identical-conda-environments) can be then used to create a conda environment using conda/mamba:

```shell
mamba create --name <env> --file <explicit spec file>
```

--8<-- [end:example]
