# `[pixi](../) run`

## About

Runs task in the pixi environment

## Usage

```text
pixi run [OPTIONS] [TASK]...

```

## Arguments

- [`<TASK>`](#arg-%3CTASK%3E) The pixi task or a task shell command you want to run in the workspace's environment, which can be an executable in the environment's PATH

  May be provided more than once.

## Options

- [`--environment (-e) <ENVIRONMENT>`](#arg---environment) The environment to run the task in
- [`--clean-env`](#arg---clean-env) Use a clean environment to run the task
- [`--skip-deps`](#arg---skip-deps) Don't run the dependencies of the task ('depends-on' field in the task definition)
- [`--dry-run (-n)`](#arg---dry-run) Run the task in dry-run mode (only print the command that would run)
- [`--help`](#arg---help) :

## Config Options

- [`--auth-file <AUTH_FILE>`](#arg---auth-file) Path to the file containing the authentication token

- [`--concurrent-downloads <CONCURRENT_DOWNLOADS>`](#arg---concurrent-downloads) Max concurrent network requests, default is `50`

- [`--concurrent-solves <CONCURRENT_SOLVES>`](#arg---concurrent-solves) Max concurrent solves, default is the number of CPUs

- [`--pinning-strategy <PINNING_STRATEGY>`](#arg---pinning-strategy) Set pinning strategy

  **options**: `semver`, `minor`, `major`, `latest-up`, `exact-version`, `no-pin`

- [`--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>`](#arg---pypi-keyring-provider) Specifies whether to use the keyring to look up credentials for PyPI

  **options**: `disabled`, `subprocess`

- [`--run-post-link-scripts`](#arg---run-post-link-scripts) Run post-link scripts (insecure)

- [`--tls-no-verify`](#arg---tls-no-verify) Do not verify the TLS certificate of the server

- [`--use-environment-activation-cache`](#arg---use-environment-activation-cache) Use environment activation cache (experimental)

- [`--force-activate`](#arg---force-activate) Do not use the environment activation cache. (default: true except in experimental mode)

- [`--no-completions`](#arg---no-completions) Do not source the autocompletion scripts from the environment

## Update Options

- [`--no-install`](#arg---no-install) Don't modify the environment, only modify the lock-file

- [`--revalidate`](#arg---revalidate) Run the complete environment validation. This will reinstall a broken environment

- [`--no-lockfile-update`](#arg---no-lockfile-update) Don't update lockfile, implies the no-install as well

- [`--frozen`](#arg---frozen) Install the environment as defined in the lockfile, doesn't update lockfile if it isn't up-to-date with the manifest file

  **env**: `PIXI_FROZEN`

- [`--locked`](#arg---locked) Check if lockfile is up-to-date before installing the environment, aborts when lockfile isn't up-to-date with the manifest file

  **env**: `PIXI_LOCKED`

## Global Options

- [`--manifest-path <MANIFEST_PATH>`](#arg---manifest-path) The path to `pixi.toml`, `pyproject.toml`, or the workspace directory

## Description

Runs task in the pixi environment.

This command is used to run tasks in the pixi environment. It will activate the environment and run the task in the environment. It is using the deno_task_shell to run the task.

`pixi run` will also update the lockfile and install the environment if it is required.

## Examples

```shell
pixi run python
pixi run cowpy "Hey pixi user"
pixi run --manifest-path ~/myworkspace/pixi.toml python
pixi run --frozen python
pixi run --locked python
# If you have specified a custom task in the pixi.toml you can run it with run as well
pixi run build
# Extra arguments will be passed to the tasks command.
pixi run task argument1 argument2
# Skip dependencies of the task
pixi run --skip-deps task
# Run in dry-run mode to see the commands that would be run
pixi run --dry-run task
# If you have multiple environments you can select the right one with the --environment flag.
pixi run --environment cuda python
# THIS DOESN'T WORK ON WINDOWS
# If you want to run a command in a clean environment you can use the --clean-env flag.
# The PATH should only contain the pixi environment here.
pixi run --clean-env "echo \$PATH"

```

## Notes

Info

In `pixi` the [`deno_task_shell`](https://deno.land/manual@v1.35.0/tools/task_runner#task-runner) is the underlying runner of the run command. Checkout their [documentation](https://deno.land/manual@v1.35.0/tools/task_runner#task-runner) for the syntax and available commands. This is done so that the run commands can be run across all platforms.

Cross environment tasks

If you're using the `depends-on` feature of the `tasks`, the tasks will be run in the order you specified them. The `depends-on` can be used cross environment, e.g. you have this `pixi.toml`:

pixi.toml

```toml
[tasks]
start = { cmd = "python start.py", depends-on = ["build"] }
[feature.build.tasks]
build = "cargo build"
[feature.build.dependencies]
rust = ">=1.74"
[environments]
build = ["build"]

```

Then you're able to run the `build` from the `build` environment and `start` from the default environment. By only calling:

```shell
pixi run start

```
