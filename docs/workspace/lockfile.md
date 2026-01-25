> A lock file is the protector of the environments, and Pixi is the key to unlock it.

## What is a lock file?

To explain this, we need to highlight the difference between the manifest and the lock file.

The manifest lists the direct dependencies of your project.
When you install your environment, this manifest goes through "dependency resolution": all the dependencies of your requested dependencies are found, et cetera all the way down.
During the resolution process, its ensured that resolved versions are compatible with each other.

A lock file lists the exact dependencies that were resolved during this resolution process - the packages, their versions, and other useful information useful for package management.

A lock file improves reproducibility as it means the project environment can easily be recreated on the same machine using this relatively small file.
Whether the lockfile can be recreated on other machines, however, depends on the package manager and whether they have cross platform support.
For example - a common problem encountered is when a package manager installs a package for a specific operating system or CPU architecture that is incompatible with other OSs or hardware.

!!! Warning "Do not edit the lock file"
    A lock file is a machine only file, and should not be edited by hand.


## Lock files in Pixi

Pixi - like many other modern package managers - has native support for lock files. This file is named `pixi.lock` .

During the creation of the lockfile, Pixi resolves the packages - for all environments, for all supported platforms.
This greatly increases the reproducibility of your project making it easy to use on different OSs or CPU architectures - in fact, for a lot of cases, sharing a lockfile can be done instead of sharing a Docker container! This is super handy for running code in CI.

The Pixi lock file is also human readable, so you can take a poke around to see which packages are listed - as well as track changes to the file (don't make edits to it - you did read the warning block above right?). 

## Lock file changes

Many Pixi commands will create a lock file if one doesn't already exist, or update it if needed. For example, when you install a package, Pixi goes through the following process:

- User requests a package to be installed
- Dependency resolution
- Generating and writing of lock file
- Install resulting packages

Additionally - Pixi ensures that the lock file remains in sync with both your manifest, as well as your installed environment.
If we detect that they aren't in sync, we will regenerate the lock file. You can read more about this in the [Lock file satisfiability](#Lock-file-satisfiability) section.

The following commands will check and automatically update the lock file if needed:

- `pixi install`
- `pixi run`
- `pixi shell`
- `pixi shell-hook`
- `pixi tree`
- `pixi list`
- `pixi add`
- `pixi remove`

If you want to remove the lock file, you can simply delete it - ready for it to be generated again with the latest package versions when one of the above commands are run.

You may want to have more control over the interplay between the manifest, the lock file, and the created environment. There are additional command line options to help with this:

- `--frozen`: install the environment as defined in the lock file, doesn't update `pixi.lock` if it isn't up-to-date with [manifest file](../reference/pixi_manifest.md). It can also be controlled by the `PIXI_FROZEN` environment variable (example: `PIXI_FROZEN=true`).
- `--locked`: only install if the `pixi.lock` is up-to-date with the [manifest file](../reference/pixi_manifest.md). It can also be controlled by the `PIXI_LOCKED` environment variable (example: `PIXI_LOCKED=true`). Conflicts with `--frozen`.



### File structure

The Pixi lock file describes the following:

- 
Within Pixi a lock file is a description of the packages in an environment.
The lock file is :

- The environments that are used in the workspace with their complete set of packages. e.g.:

  ```yaml
  environments:
      default:
          channels:
            - url: https://conda.anaconda.org/conda-forge/
          packages:
              linux-64:
              ...
              - conda: https://conda.anaconda.org/conda-forge/linux-64/python-3.12.2-hab00c5b_0_cpython.conda
              ...
              osx-64:
              ...
              - conda: https://conda.anaconda.org/conda-forge/osx-64/python-3.12.2-h9f0c242_0_cpython.conda
              ...
  ```

  - The definition of the packages themselves. e.g.:

    ```yaml
    - kind: conda
      name: python
      version: 3.12.2
      build: h9f0c242_0_cpython
      subdir: osx-64
      url: https://conda.anaconda.org/conda-forge/osx-64/python-3.12.2-h9f0c242_0_cpython.conda
      sha256: 7647ac06c3798a182a4bcb1ff58864f1ef81eb3acea6971295304c23e43252fb
      md5: 0179b8007ba008cf5bec11f3b3853902
      depends:
        - bzip2 >=1.0.8,<2.0a0
        - libexpat >=2.5.0,<3.0a0
        - libffi >=3.4,<4.0a0
        - libsqlite >=3.45.1,<4.0a0
        - libzlib >=1.2.13,<1.3.0a0
        - ncurses >=6.4,<7.0a0
        - openssl >=3.2.1,<4.0a0
        - readline >=8.2,<9.0a0
        - tk >=8.6.13,<8.7.0a0
        - tzdata
        - xz >=5.2.6,<6.0a0
      constrains:
        - python_abi 3.12.* *_cp312
      license: Python-2.0
      size: 14596811
      timestamp: 1708118065292
    ```

!!! Note "Syncing the lock file with the manifest file"
    The lock file is always matched with the whole configuration in the manifest file.
    This means that if you change the manifest file, the lock file will be updated.
    ```mermaid
    flowchart TD
        C[manifest] --> A[lock file] --> B[environment]
    ```

## Lock file satisfiability

The lock file is a description of the environment, and it should always be satisfiable.
Satisfiable means that the given manifest file and the created environment are in sync with the lock file.
If the lock file is not satisfiable, Pixi will generate a new lock file automatically.

Steps to check if the lock file is satisfiable:

- All `environments` in the manifest file are in the lock file
- All `channels` in the manifest file are in the lock file
- All `packages` in the manifest file are in the lock file, and the versions in the lock file are compatible with the requirements in the manifest file, for both `conda` and `pypi` packages.
  - Conda packages use a `matchspec` which can match on all the information we store in the lock file, even `timestamp`, `subdir` and `license`.
- If `pypi-dependencies` are added, all `conda` package that are python packages in the lock file have a `purls` field.
- All hashes for the `pypi` editable packages are correct.
- There is only a single entry for every package in the lock file.

If you want to get more details checkout the [actual code](https://github.com/prefix-dev/pixi/blob/main/src/lock_file/satisfiability/mod.rs) as this is a simplification of the actual code.

## The version of the lock file

The lock file has a version number, this is to ensure that the lock file is compatible with the local version of `pixi`.

```yaml
version: 6
```

Pixi is backward compatible with the lock file, but not forward compatible.
This means that you can use an older lock file with a newer version of `pixi`, but not the other way around.

## Your lock file is big

The lock file can grow quite large, especially if you have a lot of packages installed.
This is because the lock file contains all the information about the packages.

1. We try to keep the lock file as small as possible.
2. It's always smaller than a docker image.
3. Downloading the lock file is always faster than downloading the incorrect packages.

## You don't need a lock file because...

If you can not think of a case where you would benefit from a fast reproducible environment, then you don't need a lock file.

But take note of the following:

- A lock file allows you to run the same environment on different machines, think CI systems.
- It also allows you to go back to a working state if you have made a mistake.
- It helps other users onboard to your workspace as they don't have to figure out the environment setup or solve dependency issues.



---

Not sure what this means???

!!! Warning "Note"
    This does remove the locked state of the environment, which will be updated to the latest version of all packages.
