--8<-- [start:example]

## Examples

```shell
pixi exec python

# Add a constraint to the python version
pixi exec -s python=3.9 python

# Run ipython and include the py-rattler package in the environment
pixi exec -s ipython -s py-rattler ipython

# Force reinstall to recreate the environment and get the latest package versions
pixi exec --force-reinstall -s ipython -s py-rattler ipython
```

--8<-- [end:example]
