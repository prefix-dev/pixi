--8<-- [start:example]

## Examples

```shell
pixi task add cow cowpy "Hello User"
pixi task add tls ls --cwd tests
pixi task add test cargo t --depends-on build
pixi task add build-osx "METAL=1 cargo build" --platform osx-64
pixi task add train python train.py --feature cuda
pixi task add publish-pypi "hatch publish --yes --repo main" --feature build --env HATCH_CONFIG=config/hatch.toml --description "Publish the package to pypi"
```

This adds the following to the [manifest file](../../../pixi_manifest.md):

```toml
[tasks]
cow = "cowpy \"Hello User\""
tls = { cmd = "ls", cwd = "tests" }
test = { cmd = "cargo t", depends-on = ["build"] }

[target.osx-64.tasks]
build-osx = "METAL=1 cargo build"

[feature.cuda.tasks]
train = "python train.py"

[feature.build.tasks]
publish-pypi = { cmd = "hatch publish --yes --repo main", env = { HATCH_CONFIG = "config/hatch.toml" }, description = "Publish the package to pypi" }
```

Which you can then run with the `run` command:

```shell
pixi run cow
# Extra arguments will be passed to the tasks command.
pixi run test --test test1
```
--8<-- [end:example]
