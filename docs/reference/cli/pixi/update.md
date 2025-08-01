<!--- This file is autogenerated. Do not edit manually! -->
# <code>[pixi](../pixi.md) update</code>

## About
The `update` command checks if there are newer versions of the dependencies and updates the `pixi.lock` file and environments accordingly

--8<-- "docs/reference/cli/pixi/update_extender:description"

## Usage
```
pixi update [OPTIONS] [PACKAGES]...
```

## Arguments
- <a id="arg-<PACKAGES>" href="#arg-<PACKAGES>">`<PACKAGES>`</a>
:  The packages to update, space separated. If no packages are provided, all packages will be updated
<br>May be provided more than once.

## Options
- <a id="arg---no-install" href="#arg---no-install">`--no-install`</a>
:  Don't install the (solve) environments needed for pypi-dependencies solving
- <a id="arg---dry-run" href="#arg---dry-run">`--dry-run (-n)`</a>
:  Don't actually write the lockfile or update any environment
- <a id="arg---environment" href="#arg---environment">`--environment (-e) <ENVIRONMENTS>`</a>
:  The environments to update. If none is specified, all environments are updated
<br>May be provided more than once.
- <a id="arg---platform" href="#arg---platform">`--platform (-p) <PLATFORMS>`</a>
:  The platforms to update. If none is specified, all platforms are updated
<br>May be provided more than once.
- <a id="arg---json" href="#arg---json">`--json`</a>
:  Output the changes in JSON format

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

## Global Options
- <a id="arg---manifest-path" href="#arg---manifest-path">`--manifest-path <MANIFEST_PATH>`</a>
:  The path to `pixi.toml`, `pyproject.toml`, or the workspace directory

## Description
The `update` command checks if there are newer versions of the dependencies and updates the `pixi.lock` file and environments accordingly.

It will only update the lock file if the dependencies in the manifest file are still compatible with the new versions.


--8<-- "docs/reference/cli/pixi/update_extender:example"
