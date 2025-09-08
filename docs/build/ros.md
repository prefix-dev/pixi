This guide shows how to build a ROS package into a conda package with Pixi using the `pixi-build-ros` backend.


To understand the build feature, start with the general [Build Getting Started](./getting_started.md) guide.
For ROS without Pixi building (not packaging), see the [ROS 2 tutorial](../tutorials/ros2.md).
You may also want to read the backend documentation for [pixi-build-ros](https://prefix-dev.github.io/pixi-build-backends/backends/pixi-build-ros/).

!!! warning
    `pixi-build` is a preview feature and may change before stabilization.
    Expect rough edges; please report issues so we can improve it.

## Create a Pixi workspace

Initialize a new workspace and install the ROS 2 CLI so you can scaffold packages via the `ros2` cli.

```bash
pixi init ros_ws --channel https://prefix.dev/robostack-jazzy --channel https://prefix.dev/conda-forge
cd ros_ws
pixi add ros-jazzy-ros2run
```

This adds the `ros2` cli command to your Pixi environment.

In all examples below, ensure the [build preview](../reference/pixi_manifest.md#preview-features) is enabled in your workspace manifest:
```toml title="ros_ws/pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/pixi_build/ros_ws/pixi.toml:preview"
```

Resulting workspace manifest:
```toml title="ros_ws/pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/pixi_build/ros_ws/pixi.toml:base"
```

## Creating a Python ROS package
We'll be creating a normal ROS2 package using `ament_python` and then adding Pixi support to it.
Most of the logic is done by the ROS2 CLI, so you can follow normal ROS 2 package creation steps.

### Initialize a ROS package

Use the ROS CLI to generate an `ament_python` package skeleton within the workspace.

```bash
pixi run ros2 pkg create --build-type ament_python --destination-directory src --node-name my_python_node my_python_ros_pkg
```

You should now have something like:

```
ros_ws/
├── pixi.toml
└── src/
    └── my_python_ros_pkg/
        ├── package.xml
        ├── resource/
        ├── setup.cfg
        ├── setup.py
        ├── test/
        └── my_python_ros_pkg/
            ├── __init__.py
            └── my_python_node.py
```

### Add Pixi package info to the new package

Create a `pixi.toml` inside `src/my_python_ros_pkg` so Pixi can build it using the ROS backend. The backend reads most metadata from `package.xml`, so you only need to specify the backend and distro.

```toml title="src/my_python_ros_pkg/pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/pixi_build/ros_ws/src/my_python_ros_pkg/pixi.toml"
```

Notes:

- When `package.build.config.distro` is set, the produced package name is prefixed like `ros-<distro>-<name>`.
- The backend automatically reads `package.xml` (name, version, license, maintainers, URLs, dependencies). Any explicitly set fields in `pixi.toml` override `package.xml`.
- Dependencies in `package.xml` are mapped to conda packages via RoboStack (for example `std_msgs` → `ros-<distro>-std-msgs`). Unknown deps pass through unchanged.

### Add the package to the pixi workspace

Tell the root workspace to depend on the package via a path dependency that matches the ROS-prefixed name:

```toml title="ros_ws/pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/pixi_build/ros_ws/pixi.toml:python-pkg"
```

### Testing your package
Now install and run:

```bash
pixi run ros2 run my_python_ros_pkg my_python_node
```
Outputs:
```bash
Hi from my_python_ros_pkg.
```

## Create a CMake ROS package

Creating a C++ or mixed package using `ament_cmake`.

### Scaffold a C++ package:

```bash
pixi run ros2 pkg create --build-type ament_cmake --destination-directory src --node-name my_cmake_node my_cmake_ros_pkg
```

### Add the pixi package info

Create a `pixi.toml` inside `src/my_cmake_ros_pkg` so Pixi can build it using the ROS backend.
The backend reads most metadata from `package.xml`, so you only need to specify the `backend` and `distro`.

```toml title="src/my_cmake_ros_pkg/pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/pixi_build/ros_ws/src/my_cmake_ros_pkg/pixi.toml"
```

### Add the package to the pixi workspace

Tell the root workspace to depend on the package via a path dependency that matches the ROS-prefixed name:

```toml title="ros_ws/pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/pixi_build/ros_ws/pixi.toml:cmake-pkg"
```

### Testing your package
Now install and run:
```bash
pixi run ros2 run my_cmake_ros_pkg my_cmake_node
```
Outputs:
```bash
hello world my_cmake_ros_pkg package
```

## Building a ROS conda package
With the package(s) added to the workspace, you can now build them.

```bash
cd src/my_python_ros_pkg
pixi build
# then
cd ../my_cmake_ros_pkg
pixi build
```

You can now upload these artifacts to a conda channel and depend on them from other Pixi workspaces.

## Tips and gotchas

- ROS distro and platform: pick the correct RoboStack channel (e.g. `robostack-humble`, `robostack-jazzy`) and ensure your platform is supported.
- Keep `package.xml` accurate: name, version, license, maintainers, URLs, and dependencies are read automatically; but you can override them in the [pixi manifest](https://pixi.sh/latest/reference/pixi_manifest/#the-package-section).
- Backend docs: see the [pixi-build-ros documentation](https://prefix-dev.github.io/pixi-build-backends/backends/pixi-build-ros/) for configuration details like `env`, `distro` and `extra-input-globs`.
- Colcon vs pixi build: you don’t need `colcon` when using `pixi`; the backend invokes the right build flow. But since you don't have to change your package structure, you can still use `colcon` if you want.
- Not all ROS packages are available in RoboStack. If you depend on a package not in RoboStack, you can:
  - **Recommended:** Contribute to RoboStack to add it; see the [RoboStack Contributing page](https://robostack.github.io/Contributing.html)
  - Package it yourself with Pixi in a separate workspace and upload it to your own conda channel.
    - Optionally, this could use an [out of tree package definition](./package_source.md) to build the package without changing its source code.


## Conclusion

You can package ROS projects as conda packages with Pixi using the `pixi-build-ros` backend.
Start simple, keep `package.xml` truthful, add ROS dependencies as needed, and iterate with the preview build feature.
Once built, you can upload artifacts to a conda channel and depend on them from other Pixi workspaces.
