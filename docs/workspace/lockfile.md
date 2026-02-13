> A lock file is the protector of the environments, and Pixi is the key to unlock it.

## What is a lock file?

To answer this question, we need to highlight the difference between the manifest and the lock file.

The manifest lists the direct dependencies of your project.
When you install your environment, this manifest goes through "dependency resolution": all the dependencies of your requested dependencies are found, et cetera all the way down.
During the resolution process, it is ensured that resolved versions are compatible with each other.

A lock file lists the exact dependencies that were resolved during this resolution process - the packages, their versions, and other metadata useful for package management.

A lock file improves reproducibility as it means the project environment can easily be recreated on the same machine using this relatively small file.
Whether the lockfile can be recreated on other machines, however, depends on the package manager and whether they have cross platform support.
For example - a common problem encountered is when a package manager installs a package for a specific operating system or CPU architecture that is incompatible with other OSs or hardware.

!!! Warning "Do not edit the lock file"
    A lock file is built for machines, and made human readable for easy inspection. It's not meant to be edited by hand.

## Lock files in Pixi

Pixi - like many other modern package managers - has native support for lock files. This file is named `pixi.lock` .

During the creation of the lockfile, Pixi resolves the packages - for all environments and platforms listed in the manifest.
This greatly increases the reproducibility of your project making it easy to use on different OSs or CPU architectures - in fact, for a lot of cases, sharing a lockfile can be done instead of sharing a Docker container!
This is also super handy for running code in CI.

The Pixi lock file is also human readable, so you can take a poke around to see which packages are listed - as well as track changes to the file (don't make edits to it - you did read the warning block above right?). 

## Lock file changes

Many Pixi commands will create a lock file if one doesn't already exist, or update it if needed. For example, when you install a package, Pixi goes through the following process:

1. User requests a package to be installed
2. Dependency resolution
3. Generating and writing of lock file
4. Install resulting packages

Additionally - Pixi ensures that the lock file remains in sync with both your manifest, as well as your installed environment.
If it detects that they aren't in sync, it will regenerate the lock file. You can read more about this in the [Lock file satisfiability](#lock-file-satisfiability) section.

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


## Committing your lockfile

Reproducibility is very important in a range of projects (e.g., deploying software services, working on research projects, data analysis).
Reproducibility of environments helps with reproducibility of results - it ensures your developers, and deployment machines are all using the same packages.

Hesitant to commit the lockfile? Consider this:
- Docker images for reproducible environments are **always larger**.
- Git works well with YAML.
- It serves as a cache for the dependency resolution, giving  **faster installation and CI**.
- You don't need it... until you do. Deleting or ignoring is easier than recreating one under pressure.

There is, however, a class of projects where you may not want to commit your lock file as there are other considerations at play.
Namely, this is when developing _libraries_.

---

Libraries have an evolving nature and need to be tested against environments covering a wide range of package versions to ensure compatibility.
This includes an environment with the latest available versions of packages.

### Libraries: Committing the lockfile

If you commit the lock file in your library project, you will want to also consider the following:
- **Upgrading the lockfile:** How often do you want to upgrade the lockfile used by your developers? Do you want to do these upgrades in the main repo history? Do you want to manage this lockfile via (e.g.,) [the Renovate Bot](https://docs.renovatebot.com/modules/manager/pixi/) or via a custom CI job?
- **Custom CI workflow to test against latest versions:** Do you want to have a workflow to test against the latest dependency versions? If so - you likely want to have the following CI workflow on a cron schedule:
	- Remove the `pixi.lock` before running the `setup-pixi` action
	- Run your tests
	- If the tests fail:
		- See how the generated `pixi.lock` differs from that in `main` by using `pixi-diff` and `pixi-diff-to-markdown`
		- Automatically file an issue so that its tracked in the project repo

You can see how these considerations above have been explored by the following projects:
- Scipy (being explored - will update with PR link once available. [Issue](https://github.com/scipy/scipy/issues/23637))


### Libraries: Git-ignoring the lockfile

If you don't commit the lockfile, you end up with a simplified setup where the lockfile is generated separately for all developers, and for CI.

In CI, you can avoid the need to solve on every workflow run by caching this lockfile so that its shared between CI on the same day by using - for example - the [Parcels-code/pixi-lock](https://github.com/parcels-code/pixi-lock) action.

This simplified setup forgoes reproducibility between machines.

In both approaches, the test suite is used to determine whether the library is working as expected.

---

See the following threads for more detailed discussion on this topic:
- [prefix.dev Discord: Should you commit the lockfile](https://discord.com/channels/1082332781146800168/1462778624212996209)
- [Scientific Python Discord: lock files for libraries](https://discord.com/channels/786703927705862175/1450619697224487083)
- https://github.com/prefix-dev/pixi/issues/5325



### File structure

The Pixi lock file is structured into two parts.


- The environments that are used in the workspace - listing the packages contained. e.g.:

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
### The version of the lock file

The lock file also has a version number, this is to ensure that the lock file is compatible with the local version of `pixi`.

```yaml
version: 6
```

Pixi is backward compatible with the lock file, but not forward compatible.
This means that you can use an older lock file with a newer version of `pixi`, but not the other way around.


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

