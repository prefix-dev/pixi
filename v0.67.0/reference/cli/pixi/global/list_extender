--8<-- [start:example]

## Examples

We'll only show the dependencies and exposed binaries of the environment if they differ from the environment name.
Here is an example of a few installed packages:

```
pixi global list
```
Results in:
```
Global environments at /home/user/.pixi:
├── gh: 2.57.0
├── pixi-pack: 0.1.8
├── python: 3.11.0
│   └─ exposes: 2to3, 2to3-3.11, idle3, idle3.11, pydoc, pydoc3, pydoc3.11, python, python3, python3-config, python3.1, python3.11, python3.11-config
├── rattler-build: 0.22.0
├── ripgrep: 14.1.0
│   └─ exposes: rg
├── vim: 9.1.0611
│   └─ exposes: ex, rview, rvim, view, vim, vimdiff, vimtutor, xxd
└── zoxide: 0.9.6
```

Here is an example of list of a single environment:
```
pixi g list -e pixi-pack
```
Results in:
```
The 'pixi-pack' environment has 8 packages:
Package          Version    Build        Size
_libgcc_mutex    0.1        conda_forge  2.5 KiB
_openmp_mutex    4.5        2_gnu        23.1 KiB
ca-certificates  2024.8.30  hbcca054_0   155.3 KiB
libgcc           14.1.0     h77fa898_1   826.5 KiB
libgcc-ng        14.1.0     h69a702a_1   50.9 KiB
libgomp          14.1.0     h77fa898_1   449.4 KiB
openssl          3.3.2      hb9d3cd8_0   2.8 MiB
pixi-pack        0.1.8      hc762bcd_0   4.3 MiB
Package          Version    Build        Size

Exposes:
pixi-pack
Channels:
conda-forge
Platform: linux-64
```


--8<-- [end:example]
