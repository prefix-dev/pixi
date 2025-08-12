# `[pixi](../) upload`

## About

Upload a conda package

## Usage

```text
pixi upload <HOST> <PACKAGE_FILE>

```

## Arguments

- [`<HOST>`](#arg-%3CHOST%3E) The host + channel to upload to

  **required**: `true`

- [`<PACKAGE_FILE>`](#arg-%3CPACKAGE_FILE%3E) The file to upload

  **required**: `true`

## Description

Upload a conda package

With this command, you can upload a conda package to a channel. Example: `pixi upload https://prefix.dev/api/v1/upload/my_channel my_package.conda`

Use `pixi auth login` to authenticate with the server.
