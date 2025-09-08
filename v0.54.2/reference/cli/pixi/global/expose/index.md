# `[pixi](../../) [global](../) expose`

## About

Interact with the exposure of binaries in the global environment

## Usage

```text
pixi global expose <COMMAND>

```

## Subcommands

| Command             | Description                                                         |
| ------------------- | ------------------------------------------------------------------- |
| [`add`](add/)       | Add exposed binaries from an environment to your global environment |
| [`remove`](remove/) | Remove exposed binaries from the global environment                 |

## Description

Interact with the exposure of binaries in the global environment

`pixi global expose add python310=python3.10 --environment myenv` will expose the `python3.10` executable as `python310` from the environment `myenv`

`pixi global expose remove python310 --environment myenv` will remove the exposed name `python310` from the environment `myenv`
