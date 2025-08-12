# `[pixi](../../) [task](../) add`

## About

Add a command to the workspace

## Usage

```text
pixi task add [OPTIONS] <NAME> <COMMAND>...

```

## Arguments

- [`<NAME>`](#arg-%3CNAME%3E) Task name

  **required**: `true`

- [`<COMMAND>`](#arg-%3CCOMMAND%3E) One or more commands to actually execute

  May be provided more than once.

  **required**: `true`

## Options

- [`--depends-on <DEPENDS_ON>`](#arg---depends-on) Depends on these other commands

  May be provided more than once.

- [`--platform (-p) <PLATFORM>`](#arg---platform) The platform for which the task should be added

- [`--feature (-f) <FEATURE>`](#arg---feature) The feature for which the task should be added

- [`--cwd <CWD>`](#arg---cwd) The working directory relative to the root of the workspace

- [`--env <ENV>`](#arg---env) The environment variable to set, use --env key=value multiple times for more than one variable

  May be provided more than once.

- [`--description <DESCRIPTION>`](#arg---description) A description of the task to be added

- [`--clean-env`](#arg---clean-env) Isolate the task from the shell environment, and only use the pixi environment to run the task

- [`--arg <ARGS>`](#arg---arg) The arguments to pass to the task

  May be provided more than once.

## Examples

```shell
pixi task add cow cowpy "Hello User"
pixi task add tls ls --cwd tests
pixi task add test cargo t --depends-on build
pixi task add build-osx "METAL=1 cargo build" --platform osx-64
pixi task add train python train.py --feature cuda
pixi task add publish-pypi "hatch publish --yes --repo main" --feature build --env HATCH_CONFIG=config/hatch.toml --description "Publish the package to pypi"

```

This adds the following to the [manifest file](../../../../pixi_manifest/):

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
