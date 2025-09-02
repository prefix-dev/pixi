# `[pixi](../../) [auth](../) logout`

## About

Remove authentication information for a given host

## Usage

```text
pixi auth logout <HOST>

```

## Arguments

- [`<HOST>`](#arg-%3CHOST%3E) The host to remove authentication for

  **required**: `true`

## Examples

```shell
pixi auth logout <HOST>
pixi auth logout repo.prefix.dev
pixi auth logout anaconda.org
pixi auth logout s3://my-bucket

```
