# [pixi](../../) [auth](../) logout

Remove authentication information for a given host

## Usage

```text
pixi auth logout [OPTIONS] [HOST]
```

## Arguments

- [`<HOST>`](#arg-%3CHOST%3E) : The host to remove authentication for. With `auth-interactive` enabled, omit this (and `--all`) to pick interactively

## Options

- [`--all`](#arg---all) : Remove every stored authentication entry (revoking OAuth tokens for each)

## Examples

```shell
pixi auth logout <HOST>
pixi auth logout repo.prefix.dev
pixi auth logout anaconda.org
pixi auth logout s3://my-bucket
```
