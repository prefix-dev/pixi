# `[pixi](../) init`

## About

Creates a new workspace

Importing an environment.yml

When importing an environment, the `pixi.toml` will be created with the dependencies from the environment file. The `pixi.lock` will be created when you install the environment. We don't support `git+` urls as dependencies for pip packages and for the `defaults` channel we use `main`, `r` and `msys2` as the default channels.

## Usage

```text
pixi init [OPTIONS] [PATH]

```

## Arguments

- [`<PATH>`](#arg-%3CPATH%3E) Where to place the workspace (defaults to current path)

  **default**: `.`

## Options

- [`--channel (-c) <CHANNEL>`](#arg---channel) Channel to use in the workspace

  May be provided more than once.

- [`--platform (-p) <PLATFORM>`](#arg---platform) Platforms that the workspace supports

  May be provided more than once.

- [`--import (-i) <ENVIRONMENT_FILE>`](#arg---import) Environment.yml file to bootstrap the workspace

- [`--format <FORMAT>`](#arg---format) The manifest format to create

  **options**: `pixi`, `pyproject`, `mojoproject`

- [`--scm (-s) <SCM>`](#arg---scm) Source Control Management used for this workspace

  **options**: `github`, `gitlab`, `codeberg`

## Description

Creates a new workspace

This command is used to create a new workspace. It prepares a manifest and some helpers for the user to start working.

As pixi can both work with `pixi.toml` and `pyproject.toml` files, the user can choose which one to use with `--format`.

You can import an existing conda environment file with the `--import` flag.

## Examples

```shell
pixi init myproject  # (1)!
pixi init ~/myproject  # (2)!
pixi init  # (3)!
pixi init --channel conda-forge --channel bioconda myproject  # (4)!
pixi init --platform osx-64 --platform linux-64 myproject  # (5)!
pixi init --import environment.yml  # (6)!
pixi init --format pyproject  # (7)!
pixi init --format pixi --scm gitlab  # (8)!

```

1. Initializes a new project in the `myproject` directory, relative to the current directory.
1. Initializes a new project in the `~/myproject` directory, absolute path.
1. Initializes a new project in the current directory.
1. Initializes a new project with the specified channels.
1. Initializes a new project with the specified platforms.
1. Initializes a new project with the `dependencies` and `channels` from the `environment.yml` file.
1. Initializes a new project with the `pyproject.toml` format.
1. Initializes a new project with the `pixi.toml` format and the `gitlab` SCM.
