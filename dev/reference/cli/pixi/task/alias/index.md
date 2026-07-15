# [pixi](../../) [task](../) alias

Alias another specific command

## Usage

```text
pixi task alias [OPTIONS] <ALIAS> <DEPENDS_ON>...
```

## Arguments

- [`<ALIAS>`](#arg-%3CALIAS%3E) : Alias name

  ```
  **required**: `true`
  ```

- [`<DEPENDS_ON>`](#arg-%3CDEPENDS_ON%3E) : Depends on these tasks to execute

  ```
  May be provided more than once.
    
  **required**: `true`
  ```

## Options

- [`--platform (-p) <PLATFORM>`](#arg---platform) : The platform for which the alias should be added
- [`--environment (-e) <ENVIRONMENT>`](#arg---environment) : The environment for which the alias should be added. The alias is written to the tasks defined inline on the environment, creating the environment if it does not exist
- [`--description <DESCRIPTION>`](#arg---description) : The description of the alias task

## Examples

```shell
pixi task alias test-all test-py test-cpp test-rust
pixi task alias --platform linux-64 test test-linux
pixi task alias moo cow
pixi task alias --environment dev moo cow
```
