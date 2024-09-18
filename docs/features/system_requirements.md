# System Requirements in pixi

The system requirements are used to define minimal system specifications used during dependency resolution.
For example, we can define a unix system with a specific minimal libc version.
This will be the minimal system specification for the project.
System specifications are directly related to the [virtual packages](https://conda.io/projects/conda/en/latest/user-guide/tasks/manage-virtual.html).

### The current minimal system requirements

=== "Linux"
    ```toml title="default system requirements for linux"
    [system-requirements]
    linux = "4.18"
    libc = { family="glibc", version="2.28" }
    ```
=== "Windows"
    Windows has no minimal system requirements defined.
=== "Osx"
    ```toml title="default system requirements for osx"
    [system-requirements]
    macos = "13.0"
    ```
=== "Osx-arm64"
    ```toml title="default system requirements for osx-arm64"
    [system-requirements]
    macos = "13.0"
    ```

Only if a project requires a different set should you define them.

For example, when installing environments on old versions of linux.
You may encounter the following error:

```
× The current system has a mismatching virtual package. The project requires '__linux' to be at least version '4.18' but the system has version '4.12.14'
```

This suggests that the system requirements for the project should be lowered.
To fix this, add the following table to your configuration:

```toml
[system-requirements]
linux = "4.12.14"
```

#### Using Cuda in pixi

If you want to use `cuda` in your project you need to add the following to your `system-requirements` table:

```toml
[system-requirements]
cuda = "11" # or any other version of cuda you want to use
```

This informs the solver that cuda is going to be available, so it can lock it into the lock file if needed.

### Overriding system requirements / virtual packages
To override the logic that checks which requirements are available on your machine you can set an environment variable.
Overriding the virtual packages can be useful when you are working on a system that does not meet the requirements of the project.

Here are the available options:

- `CONDA_OVERRIDE_CUDA` - Sets the `cuda` version. e.g. `CONDA_OVERRIDE_CUDA=11`
- `CONDA_OVERRIDE_GLIBC` - Sets the `libc` version to the `glibc` family. e.g. `CONDA_OVERRIDE_GLIBC=2.28`
- `CONDA_OVERRIDE_OSX` - Sets the `macos` version. e.g. `CONDA_OVERRIDE_OSX=13.0`

The options are taken from `conda`.
For more information, see the [conda documentation](https://docs.conda.io/projects/conda/en/latest/user-guide/tasks/manage-virtual.html).
