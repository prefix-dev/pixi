# `[pixi](../) add`

## About

Adds dependencies to the workspace

## Usage

```text
pixi add [OPTIONS] <SPEC>...

```

## Arguments

- [`<SPEC>`](#arg-%3CSPEC%3E) The dependency as names, conda MatchSpecs or PyPi requirements

  May be provided more than once.

  **required**: `true`

## Options

- [`--pypi`](#arg---pypi) The specified dependencies are pypi dependencies. Conflicts with `host` and `build`

- [`--platform (-p) <PLATFORM>`](#arg---platform) The platform for which the dependency should be modified

  May be provided more than once.

- [`--feature (-f) <FEATURE>`](#arg---feature) The feature for which the dependency should be modified

  **default**: `default`

- [`--editable`](#arg---editable) Whether the pypi requirement should be editable

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

## Git Options

- [`--git (-g) <GIT>`](#arg---git) The git url to use when adding a git dependency
- [`--branch <BRANCH>`](#arg---branch) The git branch
- [`--tag <TAG>`](#arg---tag) The git tag
- [`--rev <REV>`](#arg---rev) The git revision
- [`--subdir (-s) <SUBDIR>`](#arg---subdir) The subdirectory of the git repository to use

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

Adds dependencies to the workspace

The dependencies should be defined as MatchSpec for conda package, or a PyPI requirement for the `--pypi` dependencies. If no specific version is provided, the latest version compatible with your workspace will be chosen automatically or a * will be used.

Example usage:

- `pixi add python=3.9`: This will select the latest minor version that complies with 3.9.\*, i.e., python version 3.9.0, 3.9.1, 3.9.2, etc.
- `pixi add python`: In absence of a specified version, the latest version will be chosen. For instance, this could resolve to python version 3.11.3.\* at the time of writing.

Adding multiple dependencies at once is also supported:

- `pixi add python pytest`: This will add both `python` and `pytest` to the workspace's dependencies.

The `--platform` and `--build/--host` flags make the dependency target specific.

- `pixi add python --platform linux-64 --platform osx-arm64`: Will add the latest version of python for linux-64 and osx-arm64 platforms.
- `pixi add python --build`: Will add the latest version of python for as a build dependency.

Mixing `--platform` and `--build`/`--host` flags is supported

The `--pypi` option will add the package as a pypi dependency. This cannot be mixed with the conda dependencies

- `pixi add --pypi boto3`
- `pixi add --pypi "boto3==version"`

If the workspace manifest is a `pyproject.toml`, adding a pypi dependency will add it to the native pyproject `project.dependencies` array or to the native `dependency-groups` table if a feature is specified:

- `pixi add --pypi boto3` will add `boto3` to the `project.dependencies` array
- `pixi add --pypi boto3 --feature aws` will add `boto3` to the `dependency-groups.aws` array
- `pixi add --pypi --editable 'boto3 @ file://absolute/path/to/boto3'` will add the local editable `boto3` to the `pypi-dependencies` array

Note that if `--platform` or `--editable` are specified, the pypi dependency will be added to the `tool.pixi.pypi-dependencies` table instead as native arrays have no support for platform-specific or editable dependencies.

These dependencies will then be read by pixi as if they had been added to the pixi `pypi-dependencies` tables of the default or of a named feature.

The versions will be automatically added with a pinning strategy based on semver or the pinning strategy set in the config. There is a list of packages that are not following the semver versioning scheme but will use the minor version by default: Python, Rust, Julia, GCC, GXX, GFortran, NodeJS, Deno, R, R-Base, Perl

## Examples

```shell
pixi add numpy # (1)!
pixi add numpy pandas "pytorch>=1.8" # (2)!
pixi add "numpy>=1.22,<1.24" # (3)!
pixi add --manifest-path ~/myworkspace/pixi.toml numpy # (4)!
pixi add --host "python>=3.9.0" # (5)!
pixi add --build cmake # (6)!
pixi add --platform osx-64 clang # (7)!
pixi add --no-install numpy # (8)!
pixi add --no-lockfile-update numpy # (9)!
pixi add --feature featurex numpy # (10)!
pixi add --git https://github.com/wolfv/pixi-build-examples boost-check # (11)!
pixi add --git https://github.com/wolfv/pixi-build-examples --branch main --subdir boost-check boost-check # (12)!
pixi add --git https://github.com/wolfv/pixi-build-examples --tag v0.1.0 boost-check # (13)!
pixi add --git https://github.com/wolfv/pixi-build-examples --rev e50d4a1 boost-check # (14)!
# Add a pypi dependency
pixi add --pypi requests[security] # (15)!
pixi add --pypi Django==5.1rc1 # (16)!
pixi add --pypi "boltons>=24.0.0" --feature lint # (17)!
pixi add --pypi "boltons @ https://files.pythonhosted.org/packages/46/35/e50d4a115f93e2a3fbf52438435bb2efcf14c11d4fcd6bdcd77a6fc399c9/boltons-24.0.0-py3-none-any.whl" # (18)!
pixi add --pypi "exchangelib @ git+https://github.com/ecederstrand/exchangelib" # (19)!
pixi add --pypi "project @ file:///absolute/path/to/project" # (20)!
pixi add --pypi "project@file:///absolute/path/to/project" --editable # (21)!
pixi add --git https://github.com/mahmoud/boltons.git boltons --pypi # (22)!
pixi add --git https://github.com/mahmoud/boltons.git boltons --branch main --pypi # (23)!
pixi add --git https://github.com/mahmoud/boltons.git boltons --rev e50d4a1 --pypi # (24)!
pixi add --git https://github.com/mahmoud/boltons.git boltons --tag v0.1.0 --pypi # (25)!
pixi add --git https://github.com/mahmoud/boltons.git boltons --tag v0.1.0 --pypi --subdir boltons # (26)!

```

1. This will add the `numpy` package to the project with the latest available for the solved environment.
1. This will add multiple packages to the project solving them all together.
1. This will add the `numpy` package with the version constraint.
1. This will add the `numpy` package to the project of the manifest file at the given path.
1. This will add the `python` package as a host dependency. There is currently no different behavior for host dependencies.
1. This will add the `cmake` package as a build dependency. There is currently no different behavior for build dependencies.
1. This will add the `clang` package only for the `osx-64` platform.
1. This will add the `numpy` package to the manifest and lockfile, without installing it in an environment.
1. This will add the `numpy` package to the manifest without updating the lockfile or installing it in the environment.
1. This will add the `numpy` package in the feature `featurex`.
1. This will add the `boost-check` source package to the dependencies from the git repository.
1. This will add the `boost-check` source package to the dependencies from the git repository using `main` branch and the `boost-check` folder in the repository.
1. This will add the `boost-check` source package to the dependencies from the git repository using `v0.1.0` tag.
1. This will add the `boost-check` source package to the dependencies from the git repository using `e50d4a1` revision.
1. This will add the `requests` package as `pypi` dependency with the `security` extra.
1. This will add the `pre-release` version of `Django` to the project as a `pypi` dependency.
1. This will add the `boltons` package in the feature `lint` as `pypi` dependency.
1. This will add the `boltons` package with the given `url` as `pypi` dependency.
1. This will add the `exchangelib` package with the given `git` url as `pypi` dependency.
1. This will add the `project` package with the given `file` url as `pypi` dependency.
1. This will add the `project` package with the given `file` url as an `editable` package as `pypi` dependency.
1. This will add the `boltons` package with the given `git` url as `pypi` dependency.
1. This will add the `boltons` package with the given `git` url and `main` branch as `pypi` dependency.
1. This will add the `boltons` package with the given `git` url and `e50d4a1` revision as `pypi` dependency.
1. This will add the `boltons` package with the given `git` url and `v0.1.0` tag as `pypi` dependency.
1. This will add the `boltons` package with the given `git` url, `v0.1.0` tag and the `boltons` folder in the repository as `pypi` dependency.

Tip

If you want to use a non default pinning strategy, you can set it using [pixi's configuration](../../../pixi_configuration/#pinning-strategy).

```text
pixi config set pinning-strategy no-pin --global

```

The default is `semver` which will pin the dependencies to the latest major version or minor for `v0` versions.

Note

There is an exception to this rule when you add a package we defined as non `semver`, then we'll use the `minor` strategy. These are the packages we defined as non `semver`: Python, Rust, Julia, GCC, GXX, GFortran, NodeJS, Deno, R, R-Base, Perl
