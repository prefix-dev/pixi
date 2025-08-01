<!--- This file is autogenerated. Do not edit manually! -->
# <code>[pixi](../../../pixi.md) [global](../../global.md) [expose](../expose.md) remove</code>

## About
Remove exposed binaries from the global environment

--8<-- "docs/reference/cli/pixi/global/expose/remove_extender:description"

## Usage
```
pixi global expose remove [OPTIONS] [EXPOSED_NAME]...
```

## Arguments
- <a id="arg-<EXPOSED_NAME>" href="#arg-<EXPOSED_NAME>">`<EXPOSED_NAME>`</a>
:  The exposed names that should be removed Can be specified multiple times
<br>May be provided more than once.

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

## Description
Remove exposed binaries from the global environment

`pixi global expose remove python310 python3 --environment myenv` will remove the exposed names `python310` and `python3` from the environment `myenv`


--8<-- "docs/reference/cli/pixi/global/expose/remove_extender:example"
