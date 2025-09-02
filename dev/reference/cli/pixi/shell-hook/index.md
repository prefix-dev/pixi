# `[pixi](../) shell-hook`

## About

Print the pixi environment activation script

## Usage

```text
pixi shell-hook [OPTIONS]

```

## Options

- [`--shell (-s) <SHELL>`](#arg---shell) Sets the shell, options: \[`bash`, `zsh`, `xonsh`, `cmd`, `powershell`, `fish`, `nushell`\]

- [`--environment (-e) <ENVIRONMENT>`](#arg---environment) The environment to activate in the script

- [`--json`](#arg---json) Emit the environment variables set by running the activation as JSON

  **default**: `false`

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

- [`--change-ps1 <CHANGE_PS1>`](#arg---change-ps1) Do not change the PS1 variable when starting a prompt

  **options**: `true`, `false`

## Update Options

- [`--no-install`](#arg---no-install) Don't modify the environment, only modify the lock-file

- [`--frozen=<FROZEN>`](#arg---frozen) Install the environment as defined in the lockfile, doesn't update lockfile if it isn't up-to-date with the manifest file

  **env**: `PIXI_FROZEN`

  **default**: `false`

  **options**: `true`, `false`

- [`--locked=<LOCKED>`](#arg---locked) Check if lockfile is up-to-date before installing the environment, aborts when lockfile isn't up-to-date with the manifest file

  **env**: `PIXI_LOCKED`

  **default**: `false`

  **options**: `true`, `false`

- [`--as-is`](#arg---as-is) Shorthand for the combination of --no-install and --frozen

## Global Options

- [`--manifest-path <MANIFEST_PATH>`](#arg---manifest-path) The path to `pixi.toml`, `pyproject.toml`, or the workspace directory

## Description

Print the pixi environment activation script.

You can source the script to activate the environment without needing pixi itself.

## Examples

```shell
pixi shell-hook
pixi shell-hook --shell bash
pixi shell-hook --shell zsh
pixi shell-hook -s powershell
pixi shell-hook --manifest-path ~/myworkspace/pixi.toml
pixi shell-hook --frozen
pixi shell-hook --locked
pixi shell-hook --environment cuda
pixi shell-hook --json

```

Example use-case, when you want to get rid of the `pixi` executable in a Docker container.

```shell
pixi shell-hook --shell bash > /etc/profile.d/pixi.sh
rm ~/.pixi/bin/pixi # Now the environment will be activated without the need for the pixi executable.

```
