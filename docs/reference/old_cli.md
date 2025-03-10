
### `project platform add`

Adds a platform(s) to the manifest file and updates the lock file.

##### Arguments

1. `<PLATFORM>...`: The platforms to add.

##### Options

- `--no-install`: do not update the environment, only add changed packages to the lock-file.
- `--feature <FEATURE> (-f)`: The feature for which the platform will be added.

```sh
pixi project platform add win-64
pixi project platform add --feature test win-64
```

### `project platform list`

List the platforms in the manifest file.

```sh
$ pixi project platform list
osx-64
linux-64
win-64
osx-arm64
```

### `project platform remove`

Remove platform(s) from the manifest file and updates the lock file.

##### Arguments

1. `<PLATFORM>...`: The platforms to remove.

##### Options

- `--no-install`: do not update the environment, only add changed packages to the lock-file.
- `--feature <FEATURE> (-f)`: The feature for which the platform will be removed.

```sh
pixi project platform remove win-64
pixi project platform remove --feature test win-64
```

### `project version get`

Get the project version.

```sh
$ pixi project version get
0.11.0
```

### `project version set`

Set the project version.

##### Arguments

1. `<VERSION>`: The version to set.

```sh
pixi project version set "0.13.0"
```

### `project version {major|minor|patch}`

Bump the project version to {MAJOR|MINOR|PATCH}.

```sh
pixi project version major
pixi project version minor
pixi project version patch
```

### `project system-requirement add`

Add a system requirement to the project configuration.

##### Arguments
1. `<REQUIREMENT>`: The name of the system requirement.
2. `<VERSION>`: The version of the system requirement.

##### Options
- `--family <FAMILY>`: The family of the system requirement. Only used for `other-libc`.
- `--feature <FEATURE> (-f)`: The feature for which the system requirement is added.

```shell
pixi project system-requirements add cuda 12.6
pixi project system-requirements add linux 5.15.2
pixi project system-requirements add macos 15.2
pixi project system-requirements add glibc 2.34
pixi project system-requirements add other-libc 1.2.3 --family musl
pixi project system-requirements add --feature cuda cuda 12.0
```

### `project system-requirement list`

List the system requirements in the project configuration.

##### Options
- `--environment <ENVIRONMENT> (-e)`: The environment to list the system requirements for.

```shell
pixi project system-requirements list
pixi project system-requirements list --environment test
```

[^1]:
    An **up-to-date** lock file means that the dependencies in the lock file are allowed by the dependencies in the manifest file.
    For example

    - a manifest with `python = ">= 3.11"` is up-to-date with a `name: python, version: 3.11.0` in the `pixi.lock`.
    - a manifest with `python = ">= 3.12"` is **not** up-to-date with a `name: python, version: 3.11.0` in the `pixi.lock`.

    Being up-to-date does **not** mean that the lock file holds the latest version available on the channel for the given dependency.
