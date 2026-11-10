> A lock file is the protector of the environments, and Pixi is the key to unlock it.

## What is a lock file?

To answer this question, we first need to highlight the difference between the manifest and the lock file.

The manifest lists the direct dependencies of your project.
When you install your environment, this manifest goes through "dependency resolution": all the dependencies of your requested dependencies are found, et cetera all the way down.
During the resolution process, it is ensured that resolved versions are compatible with each other.

A lock file lists the exact dependencies that were resolved during this resolution process - the packages, their versions, and other metadata useful for package management.

A lock file improves reproducibility through a relatively small file.
Installers can create exactly the same environment without needing to manage the actual package contents itself. 

!!! Warning "Do not edit the lock file"
    A lock file is built for machines, and made human readable for easy inspection. It's not meant to be edited by hand.

## Lock files in Pixi

Pixi - like many other modern package managers - has native support for lock files. This file is named `pixi.lock` .

During the creation of the lock file, Pixi resolves the packages - for all environments and platforms listed in the manifest.
This greatly increases the reproducibility of your project making it easy to use on different OSs or CPU architectures - in fact, for a lot of cases, sharing a lock file can be done instead of sharing a Docker container!
This make recreating your local development environment in CI very easy.

The Pixi lock file is also human readable, so you can take a look at which packages are listed without extra tools - as well as easily track changes to the file (don't make edits to it - you did read the warning block above right?). 

## Lock file changes

Many Pixi commands will create a lock file if one doesn't already exist, or update it if needed. For example, when you install a package, Pixi goes through the following process:

1. User requests a package to be installed
2. Dependency resolution
3. Generating and writing of lock file
4. Install resulting packages

Pixi checks whether the lock file is up to date with the manifest before installing.
If it is not, Pixi updates it unless the selected lock-file mode prevents this. See [Lock file satisfiability](#lock-file-satisfiability).

The following commands will check and automatically update the lock file if needed:

- [`pixi install`](../reference/cli/pixi/install.md)
- [`pixi run`](../reference/cli/pixi/run.md)
- [`pixi shell`](../reference/cli/pixi/shell.md)
- [`pixi shell-hook`](../reference/cli/pixi/shell-hook.md)
- [`pixi tree`](../reference/cli/pixi/tree.md)
- [`pixi list`](../reference/cli/pixi/list.md)
- [`pixi add`](../reference/cli/pixi/add.md)
- [`pixi remove`](../reference/cli/pixi/remove.md)

If you want to remove the lock file, you can simply delete it - ready for it to be generated again with the latest package versions when one of the above commands are run.

You may want to have more control over the interplay between the manifest, the lock file, and the created environment. There are additional command line options to help with this:

- `--frozen`: use the lock file without checking whether it is up to date with the [manifest file](../reference/pixi_manifest.md), and do not update it. It can also be controlled by the `PIXI_FROZEN` environment variable (example: `PIXI_FROZEN=true`).
- `--locked`: check that `pixi.lock` is up to date with the [manifest file](../reference/pixi_manifest.md), and fail rather than update it if not. It can also be controlled by the `PIXI_LOCKED` environment variable (example: `PIXI_LOCKED=true`). Conflicts with `--frozen`.
- `--no-install`: don't modify the environment, only modify the lock file. It can also be controlled by the `PIXI_NO_INSTALL` environment variable (example: `PIXI_NO_INSTALL=true`).

Neither `--locked` nor `--frozen` alone bypasses host compatibility validation.
See [Which platform gets installed?](multi_platform_configuration.md#which-platform-gets-installed) for target selection and explicit platform overrides.
The installed prefix is separate state: a fresh lock file does not mean that its packages are already installed.

## Committing your lock file

Reproducibility is very important in a range of projects (e.g., deploying software services, working on research projects, data analysis).
Reproducibility of environments helps with reproducibility of results - it ensures your developers, and deployment machines are all using the same packages.

Hesitant to commit the lock file? Consider this:

- Docker images for reproducible environments are **always larger**.
- Git works well with YAML.
- It serves as a cache for the dependency resolution, giving  **faster installation and CI**.
- You don't need it... until you do. Deleting or ignoring is easier than recreating one under pressure.

There is, however, a class of projects where one does not simply just commit the lock file - there are additional considerations at play.
Namely, this is when developing _libraries_.

---

Libraries have an evolving nature and need to be tested against environments covering a wide range of package versions to ensure compatibility.
This includes an environment with the latest available versions of packages.

### Additional considerations for libraries

If you commit the lock file in your library project, you will want to also consider the following:

- **Upgrading the lock file:** How often do you want to upgrade the lock file used by your developers? Do you want to do these upgrades in the main repo history? Do you want to manage this lock file via (e.g.,) [the Renovate Bot](https://docs.renovatebot.com/modules/manager/pixi/) or via a custom CI job?
- **Custom CI workflow to test against latest versions:** Do you want to have a workflow to test against the latest dependency versions? If so - you likely want to have the following CI workflow on a cron schedule:
  - Remove the `pixi.lock` before running the `setup-pixi` action
  - Run your tests
  - If the tests fail:
    - See how the generated `pixi.lock` differs from that in `main` by using `pixi-diff` and `pixi-diff-to-markdown`
    - Automatically file an issue so that its tracked in the project repo

You can see how these considerations above have been explored by the following projects:

- Scipy (being explored - will update with PR link once available. [Issue](https://github.com/scipy/scipy/issues/23637))

---

If you decide in the end not to commit the lock file, forgoing its benefits, you may want to use an action such as [Parcels-code/pixi-lock](https://github.com/parcels-code/pixi-lock) to cache generated lock files and speed up your CI.



### File structure

Version 7 lock files contain platform declarations, per-environment package references, and shared package records.

```yaml
version: 7
platforms:
- name: username-3
  subdir: linux-64
  virtual-packages:
  - __username_length=3
- name: username-7
  subdir: linux-64
  virtual-packages:
  - __username_length=7
environments:
  default:
    channels:
    - url: file:///example/channel/
    packages:
      username-3:
      - conda: channel/linux-64/long-username-0.1.0-0.conda
      username-7:
      - conda: channel/linux-64/long-username-0.1.0-0.conda
packages:
- conda: channel/linux-64/long-username-0.1.0-0.conda
  subdir: linux-64
  depends:
  - __username_length >=3
```

- `platforms` records each platform's name, `subdir`, and declared virtual packages.
  Names such as `username-3` are lookup keys, not subdirs. Both platforms here use the `linux-64` subdir.
- `environments.default.packages` maps those names to references into the shared `packages` records.
- `packages` describes the real packages, including their dependencies.
  Here `long-username` requires `__username_length >=3`, which both declarations satisfy.

The declared values `3` and `7` are solve inputs, not detected host values.
A host could report `__username_length=9` without that value replacing either declaration in the lock file.
Detection alone does not add a virtual package or its detector as an installed package record in the consumer environment.
See [Declared, detected, and required virtual packages](multi_platform_configuration.md#declared-detected-and-required-virtual-packages).

The serialized order of platform entries is not installation priority.
See [Which platform gets installed?](multi_platform_configuration.md#which-platform-gets-installed) for how Pixi selects and validates a target.

### The version of the lock file

The top-level `version` identifies the lock-file format, currently `7`.

Pixi is backward compatible with the lock file, but not forward compatible.
This means that you can use an older lock file with a newer version of `pixi`, but not the other way around.


## Lock file satisfiability

Lock-file satisfiability checks whether the locked resolution matches the manifest.
It is separate from validating the selected target against the host and from checking the installed prefix.
If the lock file is not satisfiable, Pixi updates it automatically unless `--locked` or `--frozen` prevents this.

Steps to check if the lock file is satisfiable:

- All `environments` in the manifest file are in the lock file
- All `channels` in the manifest file are in the lock file
- All `packages` in the manifest file are in the lock file, and the versions in the lock file are compatible with the requirements in the manifest file, for both `conda` and `pypi` packages.
  - Conda packages use a `matchspec` which can match on all the information we store in the lock file, even `timestamp`, `subdir` and `license`.
- If `pypi-dependencies` are added, all `conda` package that are python packages in the lock file have a `purls` field.
- All hashes for the `pypi` editable packages are correct.
- There is only a single entry for every package in the lock file.

If you want to get more details checkout the [actual code](https://github.com/prefix-dev/pixi/blob/main/crates/pixi_core/src/lock_file/satisfiability/mod.rs) as this is a simplification of the actual code.

