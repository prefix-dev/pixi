# pixi-build-ros

The `pixi-build-ros` backend is designed for building [ROS (Robot Operating System)](https://www.ros.org/) packages using the native ROS build systems.
No more requirement to use `colcon` or `catkin_tools` to build your ROS packages.
It provides seamless integration with Pixi's package management workflow while supporting ROS1 and ROS2 packages with automatic dependency resolution.

!!! warning
    `pixi-build` is a preview feature, and will change until it is stabilized.
    This is why we require users to opt in to that feature by adding "pixi-build" to `workspace.preview`.

    ```toml
    [workspace]
    preview = ["pixi-build"]
    ```

## Overview

This backend automatically generates conda packages from ROS projects by:

- **package.xml Integration**: Automatically reads package metadata (name, version, description, maintainers, dependencies) from your ROS `package.xml` file
- **Multi-build system support**: Supports ament_cmake, ament_python, catkin, and cmake build types
- **ROS Distribution Support**: Works with both ROS1 and ROS2 distributions (noetic, humble, jazzy, etc.)
- **Cross-platform support**: Supports Linux, macOS and Windows
- **Automatic dependency mapping**: Maps ROS dependencies to conda packages using RoboStack mappings

## Basic Usage

To use the ROS backend in your `pixi.toml`, add it to your package's build configuration:

```toml
[workspace]
preview = ["pixi-build"]
channels = [
    "https://prefix.dev/pixi-build-backends",
    "https://prefix.dev/robostack-jazzy",  # or robostack-humble, robostack-noetic, etc.
    "https://prefix.dev/conda-forge"
]
platforms = ["linux-64", "osx-arm64"]

[package.build]
backend = { name = "pixi-build-ros", version = "*" }

[package.build.config]
distro = "jazzy"  # or "humble", "noetic", etc.
```

??? Note "Workspace Configuration"
    The workspace can be defined in the `pixi.toml` of the package or in a separate `pixi.toml` at the workspace root.
    For example, with a workspace structure like this:
    ```shell
    tree -L 2
    .
    ├── pixi.toml  # Workspace configuration
    └── src
        └── my_ros_package
            ├── package.xml  # ROS package manifest
            └── pixi.toml  # Package configuration
    ```

Then you can run `pixi build` to create conda packages for your ROS packages.
```shell
pixi build
```

When you want to install it into your environment, you can do so by adding the following to your workspace `pixi.toml`:

```toml
[dependencies]
ros-jazzy-my-ros-package = { path = "." }
# or if the package is in a separate pixi.toml
# ros-jazzy-my-ros-package = { path = "src/my_ros_package" }
```
Note that you need to specify the `ros-jazzy-` prefix when you use a distro configuration.


### Automatic Metadata Detection

The backend will automatically read metadata from your `package.xml` file to populate package information **that is not** explicitly defined in your `pixi.toml`.
This includes:

- **Package name and version**: Automatically used if not specified in `pixi.toml`
- **Description**: Uses the description from `package.xml`
- **Maintainers**: Extracted from maintainer fields in `package.xml`
- **Homepage**: From URL fields with type "website" in `package.xml`
- **Repository**: From URL fields with type "repository" in `package.xml`

```xml
<package format="3">
  <name>my_ros_package</name>
  <version>1.0.0</version>
  <description>A useful ROS package for navigation</description>
  <maintainer email="developer@example.com">John Doe</maintainer>
  <url type="website">https://github.com/user/my_ros_package</url>
  <url type="repository">https://github.com/user/my_ros_package</url>
</package>
```

It would be equivalent to the following `pixi.toml`:

```toml
[package]
name = "my_ros_package"
version = "1.0.0"
description = "A useful ROS package for navigation"
maintainers = ["John Doe <developer@example.com"]
homepage = "https://github.com/user/my_ros_package"
repository = "https://github.com/user/my_ros_package"
```

The backend will automatically use the metadata from `package.xml` to generate a complete conda package named `ros-jazzy-my-ros-package`.
The fields in the `pixi.toml` will override the values from `package.xml` if they are explicitly set.

### Automatic Distro Detection
`pixi-build-ros` will automatically detect the ROS distro based on the `channels` in the workspace.
If a `distro` is not specified in the `pixi.toml`, it will be automatically detected based on the `channels` in the workspace.

```toml title="pixi.toml"
[workspace]
channels = ["conda-forge", "robostack-jazzy"]


[package.build.config]
 # This would already be automatically detected by a function in the backend.
 # Because it searches for `robostack-` and uses the first match, if it's not defined like this.
distro = "jazzy"
```

This is implemented to easily switch between distros over ros packages, by changing the `channel` used in the `workspace` section.

This does not work with the `robostack-staging` channel, as it contains packages for multiple distros.

### Automatic Dependency Resolution

Because the definition of a dependency in a `package.xml` file is not similar to a conda package name, the backend needs to map ROS dependencies to conda packages.

- **Known dependencies**: Mapped using the [`robostack.yaml`](https://github.com/prefix-dev/pixi-build-backends/blob/main/backends/pixi-build-ros/robostack.yaml).
- **Custom mappings**: You can provide additional mappings in your `pixi.toml` under [`[package.build.config.extra-package-mappings]`](#extra-package-mappings)
- **Other packages**: Mapped to `ros-<distro>-<package-name>` format.

The `<distro>` part of the package name is automatically generated based on the `distro` configuration.

## Configuration Options

You can customize the ROS backend behavior using the `[package.build.config]` section in your `pixi.toml`. The backend supports the following configuration options:

### `distro` (Optional)

- **Type**: `String`
- **Default**: Uses the [automated detection](#automatic-distro-detection) based on the `channels` in the workspace.
- **Target Merge Behavior**: `Overwrite` - Platform-specific distro takes precedence over base

The ROS distribution to build for. This affects dependency mapping and build configuration.
If set the package name will be prefixed with `ros-<distro>-` automatically, otherwise the package name from `pixi.toml` or `package.xml` is used.

```toml
[package.build.config]
distro = "jazzy"  # or "humble", "noetic", "iron", etc.
```

### `env`

- **Type**: `Map<String, String>`
- **Default**: `{}`
- **Target Merge Behavior**: `Merge` - Platform environment variables override base variables with same name, others are merged

Environment variables to set during the build process. These variables are available during compilation.

```toml
[package.build.config]
env = { AMENT_CMAKE_ENVIRONMENT_HOOKS_ENABLED = "1" }
```

#### Automatically injected environment variables

The ROS backend keeps the following variables in sync with the selected distro, so you do not need to set them manually in `env`:

- `ROS_DISTRO` &mdash; set to the distro name you configure in `distro`.
- `ROS_VERSION` &mdash; set to `"1"` for ROS 1 distros and `"2"` for ROS 2 distros.

These values are available both while evaluating `package.xml` conditionals and during the generated build script. Any custom entries you provide in `env` are merged on top of these defaults.
If you explicitly set `ROS_DISTRO` or `ROS_VERSION` in `env`, your values take precedence over the defaults.

### `debug-dir`

The backend always writes JSON-RPC request/response logs and the generated intermediate recipe to the `debug` subdirectory inside the work directory (for example `<work_directory>/debug`). The deprecated `debug-dir` configuration option is ignored; if it is still present in a manifest the backend emits a warning so you can safely remove it.

### `extra-input-globs`

- **Type**: `Array<String>`
- **Default**: `[]`
- **Target Merge Behavior**: `Overwrite` - Platform-specific globs completely replace base globs

Additional glob patterns to include as input files for the build process. These patterns are added to the default input globs that include ROS-specific files.

```toml title="pixi.toml"
[package.build.config]
extra-input-globs = [
    "launch/**/*.py",
    "config/*.yaml",
    "msgs/**/*.msg",
    "srvs/**/*.srv"
]
```

Default input globs include:
- Source files: `**/*.{c,cpp,h,hpp,rs,sh,py,pyx}`
- ROS configuration: `package.xml`, `setup.py`, `setup.cfg`, `pyproject.toml`
- Build files: `CMakeLists.txt`

### `extra-package-mappings`

- **Type**: `List<Map<String, Map<String, List<String> | RelativeFileName>>>`
- **Default**: `[]`

Additional dependency mappings to apply to the dependency mapping process.
These mappings are used to extend the usage of the dependencies in the `package.xml` file.

```toml title="pixi.toml"
[package.build.config]
extra-package-mappings = [
    {"ros-custom" = { ros =  ["ros-custom-msgs"] }},
    "mapping.yml"
]
```

Or using a toml array of tables:

```toml title="pixi.toml"
[[package.build.config.extra-package-mappings]]
custom_msgs = { ros = ["custom-messages"] }
```

Or you can use a file directly in the list:

```toml title="pixi.toml"
[package.build.config]
extra-package-mappings = ["mapping.yml"]
```

The mapping file can contain the following:

```yaml title="mapping.yml"
package_name:  # The name of the package in the package.xml
  conda: conda-package-name # Maps to a conda package, e.g. from `conda-forge`
package_name2: # The name of the package in the package.xml
  conda: [package1, package2] # Maps to a list of conda packages
ros_package:   # The name of the package in the package.xml
  ros: ros_package # Maps to a RoboStack style package name, e.g. `ros-<distro>-ros-package`
```


## Default Variants

On Windows platforms, the backend automatically sets the following default variants:

- `c_compiler`: `vs2022` - Visual Studio 2022 C compiler
- `cxx_compiler`: `vs2022` - Visual Studio 2022 C++ compiler

This default is set to align with conda-forge's switch to Visual Studio 2022 and because [mainstream support for Visual Studio 2019 ended in 2024](https://learn.microsoft.com/en-us/lifecycle/products/visual-studio-2019).
The `vs2022` compiler is more widely supported on modern GitHub runners and build environments.

You can override these defaults by explicitly setting variants using [`[workspace.build-variants]`](https://pixi.sh/latest/reference/pixi_manifest/#build-variants-optional) in your `pixi.toml`:

```toml
[workspace.build-variants]
c_compiler = ["vs2019"]
cxx_compiler = ["vs2019"]
```

## Build Process

The ROS backend follows this build process:

1. **Package Detection**: Parses `package.xml` to determine build type (`ament_cmake`, `ament_python`, `catkin`)
2. **Dependency Resolution**: Maps ROS dependencies to conda packages using RoboStack mappings
3. **Environment Setup**: Configures ROS-specific environment variables
4. **Build Execution**: Uses the appropriate build template based on package type.
5. **Installation**: Installs built artifacts to the conda package prefix

## ROS Package Types

The backend supports different ROS package build types:

### ament_cmake (ROS2)
For C++ packages using ament build system:
```xml
<export>
  <build_type>ament_cmake</build_type>
</export>
```

### ament_python (ROS2)
For Python packages using ament build system:
```xml
<export>
  <build_type>ament_python</build_type>
</export>
```

### catkin (ROS1)
For ROS1 packages using catkin build system:
```xml
<export>
  <build_type>catkin</build_type>
</export>
```

## Limitations

- **Version constraints**: Dependency versions from `package.xml` are currently ignored
- **Conditional dependencies**: Dependencies with conditions are not fully supported yet
- **Target-specific dependencies**: Platform-specific dependencies in `package.xml` need manual handling

## See Also

- [ROS Documentation](https://docs.ros.org/) - Official ROS documentation
- [RoboStack](https://robostack.github.io/) - Conda packages for the Robot Operating System
- [ament Build System](https://docs.ros.org/en/rolling/Concepts/Build-System-Development/ament.html) - ROS2 build system
- [catkin Build System](http://wiki.ros.org/catkin) - ROS1 build system
