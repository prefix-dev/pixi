<!--- This file is autogenerated. Do not edit manually! -->
# <code>[pixi](../../../pixi.md) [global](../../global.md) [expose](../expose.md) add</code>

## About
Add exposed binaries from an environment to your global environment

--8<-- "docs/reference/cli/pixi/global/expose/add_extender:description"

## Usage
```
pixi global expose add [OPTIONS] --environment <ENVIRONMENT> [MAPPING]...
```

## Arguments
- <a id="arg-<MAPPING>" href="#arg-<MAPPING>">`<MAPPING>`</a>
:  Add mapping which describe which executables are exposed. The syntax is `exposed_name=executable_name`, so for example `python3.10=python`. Alternatively, you can input only an executable_name and `executable_name=executable_name` is assumed
<br>May be provided more than once.

## Options
- <a id="arg---environment" href="#arg---environment">`--environment (-e) <ENVIRONMENT>`</a>
:  The environment to which the binaries should be exposed
<br>**required**: `true`

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
Add exposed binaries from an environment to your global environment

Example:

- `pixi global expose add python310=python3.10 python3=python3 --environment myenv`
- `pixi global add --environment my_env pytest pytest-cov --expose pytest=pytest`


--8<-- "docs/reference/cli/pixi/global/expose/add_extender:example"
