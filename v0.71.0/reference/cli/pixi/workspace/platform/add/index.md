# [pixi](../../../) [workspace](../../) [platform](../) add

Adds a platform(s) to the workspace file and updates the lock file

## Usage

```text
pixi workspace platform add [OPTIONS] <PLATFORM|NAME=PLATFORM|__NAME[=VERSION[=BUILD]]>...
```

## Arguments

- [`<PLATFORM|NAME=PLATFORM|__NAME[=VERSION[=BUILD]]>`](#arg-%3CPLATFORM%7CNAME=PLATFORM%7C__NAME%5B=VERSION%5B=BUILD%5D%5D%3E) : Platforms to add, optionally followed by raw virtual-package specs

  ```
  May be provided more than once.
    
  **required**: `true`
  ```

## Options

- [`--cuda <VERSION>`](#arg---cuda) : Declare a `__cuda` virtual package at the given version, e.g. `12.0`. Valid on any subdir

- [`--cuda-arch <VERSION>`](#arg---cuda-arch) : Declare a `__cuda_arch` virtual package (GPU compute capability) at the given version, e.g. `8.6`. Requires `--cuda` (or an existing `__cuda`), matching the conda CEP coupling. Serialized as `cuda = { driver, arch }`

- [`--archspec <ARCH>`](#arg---archspec) : Declare a `__archspec` virtual package with the given microarchitecture string, e.g. `x86-64-v3`. Valid on any subdir

- [`--glibc <VERSION>`](#arg---glibc) : Declare a `__glibc` virtual package at the given version, e.g. `2.28`. Only valid on linux subdirs

- [`--linux <VERSION>`](#arg---linux) : Declare a `__linux` virtual package at the given kernel version, e.g. `5.10`. Only valid on linux subdirs

- [`--macos <VERSION>`](#arg---macos) : Declare a `__osx` virtual package at the given macOS version, e.g. `14.0`. Only valid on osx subdirs

  ```
  **aliases**: osx
  ```

- [`--windows <VERSION>`](#arg---windows) : Declare a `__win` virtual package at the given Windows version, e.g. `10`. Only valid on win subdirs

- [`--no-install`](#arg---no-install) : Don't update the environment, only add changed packages to the lock file

  ```
  **env**: `PIXI_NO_INSTALL`
  ```

- [`--feature (-f) <FEATURE>`](#arg---feature) : The name of the feature to add the platform to
