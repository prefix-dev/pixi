# <code>[pixi](../pixi.md) update</code>

## About
Update dependencies as recorded in the local lock file

--8<-- "docs/reference/cli/pixi/update_extender.md:description"

## Usage
```
pixi update [OPTIONS] [PACKAGES]...
```

## Arguments
- <a id="arg-<PACKAGES>" href="#arg-<PACKAGES>">`<PACKAGES>`</a>
:  The packages to update

## Options
- <a id="arg---auth-file" href="#arg---auth-file">`--auth-file <AUTH_FILE>`</a>
:  Path to the file containing the authentication token
- <a id="arg---concurrent-downloads" href="#arg---concurrent-downloads">`--concurrent-downloads <CONCURRENT_DOWNLOADS>`</a>
:  Max concurrent network requests, default is 50
- <a id="arg---concurrent-solves" href="#arg---concurrent-solves">`--concurrent-solves <CONCURRENT_SOLVES>`</a>
:  Max concurrent solves, default is the number of CPUs
- <a id="arg---dry-run" href="#arg---dry-run">`--dry-run (-n)`</a>
:  Don't actually write the lockfile or update any environment
- <a id="arg---environment" href="#arg---environment">`--environment (-e) <ENVIRONMENTS>`</a>
:  The environments to update. If none is specified, all environments are updated
- <a id="arg---json" href="#arg---json">`--json`</a>
:  Output the changes in JSON format
- <a id="arg---no-install" href="#arg---no-install">`--no-install`</a>
:  Don't install the (solve) environments needed for pypi-dependencies solving
- <a id="arg---platform" href="#arg---platform">`--platform (-p) <PLATFORMS>`</a>
:  The platforms to update. If none is specified, all platforms are updated
- <a id="arg---pypi-keyring-provider" href="#arg---pypi-keyring-provider">`--pypi-keyring-provider <PYPI_KEYRING_PROVIDER>`</a>
:  Specifies if we want to use uv keyring provider
<br>**options**: `disabled`, `subprocess`
- <a id="arg---tls-no-verify" href="#arg---tls-no-verify">`--tls-no-verify`</a>
:  Do not verify the TLS certificate of the server

## Global Options
- <a id="arg---manifest-path" href="#arg---manifest-path">`--manifest-path <MANIFEST_PATH>`</a>
:  The path to `pixi.toml`, `pyproject.toml`, or the project directory

--8<-- "docs/reference/cli/pixi/update_extender.md:example"
