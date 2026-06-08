# [pixi](../../../) [workspace](../../) [platform](../) edit

Edit an existing workspace platform's subdir and/or virtual packages

## Usage

```text
pixi workspace platform edit [OPTIONS] <NAME> [__NAME[=VERSION[=BUILD]]]...
```

## Arguments

- [`<NAME>`](#arg-%3CNAME%3E) : Name of the platform to edit

  ```
  **required**: `true`
  ```

- <a id="arg-<__NAME[=VERSION[=BUILD]]>" href="#arg-<__NAME[=VERSION[=BUILD]]>">`<__NAME[=VERSION[=BUILD]]>` : Raw virtual-package specs (`__name[=version[=build_string]]`) to declare or update on this platform. Use the friendly flags (`--cuda`, `--archspec`, ...) for virtual packages that have one; this trailing positional list is the escape hatch for everything else, mirroring the `__name = "..."` raw keys accepted in pixi.toml

  ```
  May be provided more than once.
  ```

## Options

- [`--subdir <SUBDIR>`](#arg---subdir) : Set a new conda subdir for this platform

- [`--cuda <VERSION>`](#arg---cuda) : Declare a `__cuda` virtual package at the given version, e.g. `12.0`. Valid on any subdir

- [`--archspec <ARCH>`](#arg---archspec) : Declare a `__archspec` virtual package with the given microarchitecture string, e.g. `x86-64-v3`. Valid on any subdir

- [`--glibc <VERSION>`](#arg---glibc) : Declare a `__glibc` virtual package at the given version, e.g. `2.28`. Only valid on linux subdirs

- [`--linux <VERSION>`](#arg---linux) : Declare a `__linux` virtual package at the given kernel version, e.g. `5.10`. Only valid on linux subdirs

- [`--macos <VERSION>`](#arg---macos) : Declare a `__osx` virtual package at the given macOS version, e.g. `14.0`. Only valid on osx subdirs

  ```
  **aliases**: osx
  ```

- [`--windows <VERSION>`](#arg---windows) : Declare a `__win` virtual package at the given Windows version, e.g. `10`. Only valid on win subdirs

- [`--remove-virtual-package <NAME>`](#arg---remove-virtual-package) : Remove the named virtual package from this platform. Can be repeated

  ```
  May be provided more than once.
  ```

- [`--clear-virtual-packages`](#arg---clear-virtual-packages) : Clear all virtual packages before applying any add/upsert operations

- [`--no-install`](#arg---no-install) : Don't update the environment, only refresh the lock-file

  ```
  **env**: `PIXI_NO_INSTALL`
  ```
