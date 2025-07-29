**System requirements** tell Pixi the system specifications needed to install and run your workspace’s environment.
They ensure that the dependencies match the operating system and hardware of your machine.

!!! note "Think of it like this:"
    You’re defining what “kind of machines” your workspace can run on.
    ```toml
    [system-requirements]
    linux  = "4.18"
    libc   = { family = "glibc", version = "2.28" }
    cuda   = "12"
    macos  = "13.0"
    ```
    This results in a workspace that can run on:

    - Linux with kernel version `4.18`
    - GNU C Library (glibc) version `2.28`
    - CUDA version `12`
    - macOS version `13.0`


When resolving dependencies, Pixi combines:

- The default requirements for the `platforms`.
- Any custom requirements you’ve added for your workspace through the `[system-requirements]`.

This way, Pixi guarantees your environment is consistent and compatible with your machine.

System specifications are closely related to [virtual packages](https://conda.io/projects/conda/en/latest/user-guide/tasks/manage-virtual.html), allowing for flexible and accurate dependency management.

!!! note "Need to support multiple types of systems that don't share the same specifications?"
    You can define `system-requirements` for different `features` in your workspace.
    For example, if you have a feature that requires CUDA and another that does not, you can specify the system requirements for each feature separately.
    Check the example [below](#setting-system-requirements-environment-specific) for more details.


## Maximum or Minimum System Requirements
The system requirements don't specify a maximum or minimum version.
They specify the version that can be expected on the host system.
It's up to the dependency resolver to determine if the system meets the requirements based on the versions available.
e.g.:

- a package can require `__cuda >= 12` and the system can have `12.1`, `12.6`, or any higher version.
- a package can require `__cuda <= 12` and the system can have `12.0.0`, `11`, or any lower version.

Most of the time, packages will specify the minimum version (`>=`) it requires.
So we often say that the `system-requirements` define the minimum version of the system specifications.

For example [`cuda-version-12.9-h4f385c5_3.conda`](https://conda-metadata-app.streamlit.app/?q=conda-forge%2Fnoarch%2Fcuda-version-12.9-h4f385c5_3.conda)
contains the following package constraints:

```
cudatoolkit 12.9|12.9.*
__cuda >=12
```

## Default System Requirements

The following configurations outline the default system requirements for different operating systems:

=== "Linux"
    ```toml
    # Default system requirements for Linux
    [system-requirements]
    linux = "4.18"
    libc = { family = "glibc", version = "2.28" }
    ```
=== "Windows"
    Windows currently has no minimal system requirements defined. If your workspace requires specific Windows configurations,
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

You only need to define system requirements if your workspace necessitates a different set from the defaults.
This is common when installing environments on older or newer versions of operating systems.

### Adjusting for Older Systems
If you're encountering an error like:

```bash
× The current system has a mismatching virtual package. The workspace requires '__linux' to be at least version '4.18' but the system has version '4.12.14'
```

This indicates that the workspace's system requirements are higher than your current system's specifications.
To resolve this, you can lower the system requirements in your workspace's configuration:

```toml
[system-requirements]
linux = "4.12.14"
```

This adjustment informs the dependency resolver to accommodate the older system version.

### Using CUDA in pixi

To utilize CUDA in your workspace, you must specify the desired CUDA version in the system-requirements table.
This ensures that CUDA is recognized and appropriately locked into the lock file if necessary.

Example Configuration

```toml
[system-requirements]
cuda = "12"  # Replace "12" with the specific CUDA version you intend to use
```

1. Can `system-requirements` enforce a specific CUDA runtime version?
    - No. The `system-requirements` field is used to specify the supported CUDA version based on the host’s NVIDIA driver API.
Adding this field ensures that packages depending on `__cuda >= {version}` are resolved correctly.

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
This can be particularly useful when working on systems that do not meet the workspace's default requirements.

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
