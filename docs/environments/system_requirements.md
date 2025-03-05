# System Requirements in pixi
**System requirements** tell pixi the minimum system specifications needed to install and run your project’s environment.
They ensure that the dependencies match the operating system and hardware of your machine.

Think of it like this:
You’re defining what “kind of computer” your project can run on.
For example, you might require a specific Linux kernel version or a minimum glibc version.
If your machine doesn’t meet these requirements, `pixi run` will fail because the environment can’t work reliably on your system.

When resolving dependencies, pixi combines:

- The default requirements for the operating system.
- Any custom requirements you’ve added for your project through the `[system-requirements]`.

This way, pixi guarantees your environment is consistent and compatible with your machine.

System specifications are closely related to [virtual packages](https://conda.io/projects/conda/en/latest/user-guide/tasks/manage-virtual.html), allowing for flexible and accurate dependency management.

## Default System Requirements

The following configurations outline the default minimal system requirements for different operating systems:

=== "Linux"
    ```toml
    # Default system requirements for Linux
    [system-requirements]
    linux = "4.18"
    libc = { family = "glibc", version = "2.28" }
    ```
=== "Windows"
    Windows currently has no minimal system requirements defined. If your project requires specific Windows configurations,
    you should define them accordingly.
=== "osx-64"
    ```toml
    # Default system requirements for macOS
    [system-requirements]
    macos = "13.0"
    ```
=== "osx-arm64"
    ```toml
    # Default system requirements for macOS ARM64
    [system-requirements]
    macos = "13.0"
    ```

## Customizing System Requirements

You only need to define system requirements if your project necessitates a different set from the defaults.
This is common when installing environments on older or newer versions of operating systems.

### Adjusting for Older Systems
If you're encountering an error like:

```bash
× The current system has a mismatching virtual package. The project requires '__linux' to be at least version '4.18' but the system has version '4.12.14'
```

This indicates that the project's system requirements are higher than your current system's specifications.
To resolve this, you can lower the system requirements in your project's configuration:

```toml
[system-requirements]
linux = "4.12.14"
```

This adjustment informs the dependency resolver to accommodate the older system version.

### Using CUDA in pixi

To utilize CUDA in your project, you must specify the desired CUDA version in the system-requirements table.
This ensures that CUDA is recognized and appropriately locked into the lock file if necessary.

Example Configuration

```toml
[system-requirements]
cuda = "12"  # Replace "12" with the specific CUDA version you intend to use
```

1. Can `system-requirements` enforce a specific CUDA runtime version?
    - No. The `system-requirements` field is used to specify the maximum supported CUDA version based on the host’s NVIDIA driver API.
Adding this field ensures that packages depending on `__cuda >= {version}` are resolved correctly.
2.

### Setting System Requirements environment specific
This can be set per `feature` in the `the manifest` file.

```toml
[feature.cuda.system-requirements]
cuda = "12"

[environments]
cuda = ["cuda"]
```

### Available Override Options
In certain scenarios, you might need to override the system requirements detected on your machine.
This can be particularly useful when working on systems that do not meet the project's default requirements.

You can override virtual packages by setting the following environment variables:

- `CONDA_OVERRIDE_CUDA`
  - Description: Sets the CUDA version.
  - Usage Example: `CONDA_OVERRIDE_CUDA=11`
- `CONDA_OVERRIDE_GLIBC`
  - Description: Sets the glibc version.
  - Usage Example: `CONDA_OVERRIDE_GLIBC=2.28`
- `CONDA_OVERRIDE_OSX`
  - Description: Sets the macOS version.
  - Usage Example: `CONDA_OVERRIDE_OSX=13.0`

## Additional Resources

For more detailed information on managing `virtual packages` and overriding system requirements, refer to
the [Conda Documentation](https://docs.conda.io/projects/conda/en/latest/user-guide/tasks/manage-virtual.html).
