<!--- This file is autogenerated. Do not edit manually! -->
# <code>[pixi](../pixi.md) run</code>

## About
Runs task in the pixi environment

--8<-- "docs/reference/cli/pixi/run_extender:description"

## Usage
```
pixi run [OPTIONS] [TASK]...
```

## Arguments
- <a id="arg-<TASK>" href="#arg-<TASK>">`<TASK>`</a>
:  The pixi task or a task shell command you want to run in the workspace's environment, which can be an executable in the environment's PATH
<br>May be provided more than once.

## Options
- <a id="arg---environment" href="#arg---environment">`--environment (-e) <ENVIRONMENT>`</a>
:  The environment to run the task in
- <a id="arg---clean-env" href="#arg---clean-env">`--clean-env`</a>
:  Use a clean environment to run the task
- <a id="arg---skip-deps" href="#arg---skip-deps">`--skip-deps`</a>
:  Don't run the dependencies of the task ('depends-on' field in the task definition)
- <a id="arg---dry-run" href="#arg---dry-run">`--dry-run (-n)`</a>
:  Run the task in dry-run mode (only print the command that would run)
- <a id="arg---help" href="#arg---help">`--help`</a>
:

## Config Options
- <a id="arg---auth-file" href="#arg---auth-file">`--auth-file <AUTH_FILE>`</a>
:  Path to the file containing the authentication token
- <a id="arg---concurrent-downloads" href="#arg---concurrent-downloads">`--concurrent-downloads <CONCURRENT_DOWNLOADS>`</a>
:  Max concurrent network requests, default is `50`
- <a id="arg---concurrent-solves" href="#arg---concurrent-solves">`--concurrent-solves <CONCURRENT_SOLVES>`</a>
:  Max concurrent solves, default is the number of CPUs
- <a id="arg---pinning-strategy" href="#arg---pinning-strategy">`--pinning-strategy <PINNING_STRATEGY>`</a>
:  Set pinning strategy
<br>**options**: `semver`, `minor`, `major`, `latest-up`, `exact-version`, `no-pin`
- <a id="arg---pypi-keyring-provider" href="#arg---pypi-keyring-provider">`--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>`</a>
:  Specifies whether to use the keyring to look up credentials for PyPI
<br>**options**: `disabled`, `subprocess`
- <a id="arg---run-post-link-scripts" href="#arg---run-post-link-scripts">`--run-post-link-scripts`</a>
:  Run post-link scripts (insecure)
- <a id="arg---tls-no-verify" href="#arg---tls-no-verify">`--tls-no-verify`</a>
:  Do not verify the TLS certificate of the server
- <a id="arg---use-environment-activation-cache" href="#arg---use-environment-activation-cache">`--use-environment-activation-cache`</a>
:  Use environment activation cache (experimental)
- <a id="arg---force-activate" href="#arg---force-activate">`--force-activate`</a>
:  Do not use the environment activation cache. (default: true except in experimental mode)
- <a id="arg---no-completions" href="#arg---no-completions">`--no-completions`</a>
:  Do not source the autocompletion scripts from the environment

## Update Options
- <a id="arg---no-install" href="#arg---no-install">`--no-install`</a>
:  Don't modify the environment, only modify the lock-file
- <a id="arg---revalidate" href="#arg---revalidate">`--revalidate`</a>
:  Run the complete environment validation. This will reinstall a broken environment
- <a id="arg---no-lockfile-update" href="#arg---no-lockfile-update">`--no-lockfile-update`</a>
:  Don't update lockfile, implies the no-install as well
- <a id="arg---frozen" href="#arg---frozen">`--frozen`</a>
:  Install the environment as defined in the lockfile, doesn't update lockfile if it isn't up-to-date with the manifest file
<br>**env**: `PIXI_FROZEN`
- <a id="arg---locked" href="#arg---locked">`--locked`</a>
:  Check if lockfile is up-to-date before installing the environment, aborts when lockfile isn't up-to-date with the manifest file
<br>**env**: `PIXI_LOCKED`

## Global Options
- <a id="arg---manifest-path" href="#arg---manifest-path">`--manifest-path <MANIFEST_PATH>`</a>
:  The path to `pixi.toml`, `pyproject.toml`, or the workspace directory

## Description
Runs task in the pixi environment.

This command is used to run tasks in the pixi environment. It will activate the environment and run the task in the environment. It is using the deno_task_shell to run the task.

`pixi run` will also update the lockfile and install the environment if it is required.


--8<-- "docs/reference/cli/pixi/run_extender:example"
