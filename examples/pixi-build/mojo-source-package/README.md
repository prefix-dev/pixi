# Mojo source and precompiled package variants

This example exposes the same `mojo-math` project in two formats from one invocation of `pixi-build-mojo`:

- `mojo:source`: a generic `noarch` package containing the `.mojo` source tree.
- `mojo:precompiled`: a generic `noarch` `.mojopkg` tied to the exact `mojo-compiler` version used to create it.

`main.mojo` imports either installed variant normally:

```mojo
from mojo_math import answer
```

Run the preferred variant, which is source because the backend down-prioritizes precompiled output:

```bash
./examples/pixi-build/mojo-source-package/run.sh
```

Select either variant explicitly through its package flag:

```bash
./examples/pixi-build/mojo-source-package/run.sh source
./examples/pixi-build/mojo-source-package/run.sh precompiled
```

`run.sh` builds both Pixi and `pixi-build-mojo` from this checkout. It sets `PIXI_BUILD_BACKEND_OVERRIDE` to the backend's absolute path, ensuring the example exercises the implementation in this repository.
