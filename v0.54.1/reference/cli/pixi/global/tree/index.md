# `[pixi](../../) [global](../) tree`

## About

Show a tree of dependencies for a specific global environment

## Usage

```text
pixi global tree [OPTIONS] --environment <ENVIRONMENT> [REGEX]

```

## Arguments

- [`<REGEX>`](#arg-%3CREGEX%3E) List only packages matching a regular expression

## Options

- [`--environment (-e) <ENVIRONMENT>`](#arg---environment) The environment to list packages for

  **required**: `true`

- [`--invert (-i)`](#arg---invert) Invert tree and show what depends on a given package in the regex argument

## Description

Show a tree of a global environment dependencies

Dependency names highlighted in green are directly specified in the manifest.
