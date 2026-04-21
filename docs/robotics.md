# Pixi for Robotics

**Pixi is not only for robotics**, but it's a great fit for it!

Pixi is a fast, reproducible package manager built on the conda ecosystem, and a natural fit for robotics development.

If you're familiar with the traditional ROS setup, here's the mental model:

- **`apt`** installs **Debian packages** from the **Ubuntu distribution**. The **official ROS apt repository** (`packages.ros.org`) is an additional channel built on top of it.
- **Pixi** installs **conda packages** from the **[conda-forge](https://conda-forge.org) distribution**, **[RoboStack](https://robostack.github.io/)** is an additional conda channel built on top of it in exactly the same way: it repackages the ROS ecosystem as conda packages, resolving all native dependencies against `conda-forge`.

The structure is identical: a base distribution (`Ubuntu` / `conda-forge`) with a ROS channel layered on top (`packages.ros.org` / `robostack-<distro>`).
The key difference is that conda packages are cross-platform and not tied to a specific OS, so the same `pixi.toml` can work on Linux, macOS, and Windows without a system-wide install.

Pixi and RoboStack are currently a Tier 3 platform in the ROS ecosystem, which means they are community-supported but not officially supported by Open Robotics.
[Open Robotics has communicated](https://www.openrobotics.org/technology-strategy-2026) that Pixi will be part of their strategy to expand accessibility and ease of use.
So with your help, we can grow the community and make it a first-class way to use ROS on any platform.

---

## Quick Start

Get ROS 2 Humble running in no time:

```shell
pixi init my_ros_ws -c https://prefix.dev/robostack-humble -c https://prefix.dev/conda-forge
cd my_ros_ws
pixi add ros-humble-desktop
pixi run rviz2
```

ROS 2 is installed, isolated to this workspace, and reproducible on any machine with Pixi.
A `pixi.toml` file is created with the dependencies and channels, and a `pixi.lock` file is generated to pin exact versions.
Use `pixi run` or `pixi shell` to activate the environment and run ROS commands.

---

## Why Pixi for Robotics

- [x] **No system-wide ROS installation** <br>
    All dependencies live in the workspace. New team members clone the repo and run `pixi install`. No more additional setup instructions, no version incompatibilities.
- [x] **[Reproducible environments](workspace/lockfile.md)** <br>
    Pixi generates a lockfile (`pixi.lock`) that pins every dependency, including transitive ones. The same environment is guaranteed on your laptop, CI, and the robot itself.
- [x] **[Works on Linux, macOS, and Windows](workspace/multi_platform_configuration.md)** <br>
    RoboStack supports ROS packages for Linux, macOS and Windows. A single `pixi.toml` can declare support for all three platforms simultaneously.
- [x] **[Built-in task runner](workspace/advanced_tasks.md)** <br>
    Define your `colcon build`, launch files, and simulation commands as named tasks in `pixi.toml`, no Makefiles or shell scripts needed.
- [x] **Mix ROS and non-ROS dependencies** <br>
    Python libraries, OpenCV, PyTorch, compilers, GUI's and ROS packages all managed together.
- [x] **[Package and distribute your own nodes](build/ros.md)** <br>
    Build your ROS packages as conda packages with the [`pixi-build-ros`](build/backends/pixi-build-ros.md) backend and publish them to a private channel.

---

## How Pixi Compares

|                                      | `apt` | `pip`    | Docker               | **Pixi** |
|--------------------------------------|-------|----------|----------------------|----------|
| Installs ROS                         | ✅     | ❌        | ✅                    | ✅        |
| Works on macOS & Windows             | ❌     | ✅        | ✅                    | ✅        |
| Reproducible lockfile                | ❌     | ❌        | ⚠️(pre-baked images) | ✅        |
| Per-project isolation                | ❌     | ⚠️ venvs | ✅                    | ✅        |
| Mix C++, Python, system libs         | ✅     | ❌        | ✅                    | ✅        |
| Native performance (no VM/container) | ✅     | ✅        | ❌                    | ✅        |
| GUI / hardware access out of the box | ✅     | ✅        | ⚠️ (requires setup)  | ✅        |
| Single install command for new devs  | ❌     | ❌        | ⚠️                   | ✅        |

- `apt` is Debian-only and install packages globally, making it hard to run different ROS versions side by side or reproduce the exact same setup across machines.
- `pip` can't install ROS or non-Python system libraries at all.
- Docker gives you isolation and improves cross-platform support but adds overhead, complicates GPU and hardware access, and requires a separate toolchain.
- Pixi gives you the package isolation and reproducibility of Docker with the native performance and hardware access of a local install, on any platform.

---

## Guides

| Guide | Description |
|---|---|
| [ROS 2 Tutorial](tutorials/ros2.md) | Set up a workspace, write Python and C++ nodes, use `pixi run` tasks. |
| [Building a ROS Package](build/ros.md) | Package your ROS nodes as conda packages with `pixi-build-ros`. |
| [pixi-build-ros Backend](build/backends/pixi-build-ros.md) | Backend reference for building ROS packages. |
| [GitHub Actions](integration/ci/github_actions.md) | Run your ROS tests in CI. |
| [Distributing with Pixi Pack](deployment/pixi_pack.md) | Bundle a workspace for offline or embedded deployment. |

---

## RoboStack

[RoboStack](https://robostack.github.io/) is the community that maintains the conda ROS packages Pixi depends on.

#### Finding packages 

Browse available packages for each distribution on [prefix.dev](https://prefix.dev/channels/).
Package names follow the pattern `ros-<distro>-<package-name>`, mirroring the apt naming convention.
There are more packages on the `prefix.dev` channels so some might normally come from `apt` or `pip` instead, but they are all available in a single place for Pixi:

| `pixi`                      | `apt`                          | `pip`                                                                               |
|-----------------------------|--------------------------------|-------------------------------------------------------------------------------------|
| `pixi add ros-humble-rviz2` | `apt install ros-humble-rviz2` | ❌                                                                                   |
| `pixi add opencv`           | `apt install python3-opencv`   | `pip install opencv-python`                                                         |
| `pixi add pytorch`          | ❌                              | `pip install torch --index-url https://download.pytorch.org/whl/cpu`    |

#### What to do if a package is missing

Not all ROS packages are available in RoboStack yet. If you need a package that is not in the channel:

- Check the [RoboStack GitHub](https://github.com/RoboStack) to see if there is an open issue or PR for it.
- Contribute to RoboStack to add it. See the [RoboStack contributing guide](https://robostack.github.io/Contributing.html).
- Package it yourself with Pixi using the [`pixi-build-ros` backend](build/backends/pixi-build-ros.md) and publish it to your own conda channel.

#### `rosdep` is not supported

`rosdep` calls `apt` or `pip` under the hood, which bypasses Pixi's environment.
Use `pixi add` to add any missing dependencies instead.

#### `pixi-ros`

[`pixi-ros`](https://github.com/ruben-arts/pixi-ros) can be used to quickstart a ROS workspace.
This project can initialize a workspace with the correct RoboStack channels and ROS dependencies, and generate `pixi.toml` files for ROS packages.

```shell
# Install the pixi-ros CLI tool globally
pixi global install pixi-ros
# Move to your existing ros workspace that contains the src folder
cd ros_ws
# Initialize the workspace
pixi ros init
```

It will take you through some setup and help you initialize your first Pixi ROS workspace.

---

## Common Patterns

### Multi-machine workspace

Share the exact same environment across development machines and the robot:

```toml title="pixi.toml"
[workspace]
channels = ["https://prefix.dev/robostack-humble", "https://prefix.dev/conda-forge"]
platforms = ["linux-64", "osx-arm64"]

[dependencies]
ros-humble-desktop = "*"
colcon-common-extensions = "*"
```

### Using Pixi with colcon
Assuming you have a workspace with a `src` folder containing your ROS packages, you can define the following manifest in the root of your workspace:
```toml title="pixi.toml"
[workspace]
channels = ["https://prefix.dev/robostack-humble", "https://prefix.dev/conda-forge"]
platforms = ["linux-64", "osx-arm64", "win-64"]

[dependencies]
ros-humble-desktop = "*"
colcon-common-extensions = "*"

[tasks]
# A simple colcon build task
build = "colcon build --symlink-install"

[activation]
# After you use colcon to build your workspace, Pixi has to source the setup script to make the new packages available in the environment
scripts = ["install/setup.sh"]
```

The activation scripts will be run on `pixi run` or `pixi shell`, so after the first build, you can run your ROS nodes with `pixi run` without needing to source the setup script manually.


### Automate your workflow with tasks

```toml title="pixi.toml"
[tasks]
# A simple alias for running the turtlesim simulator
sim   = "ros2 run turtlesim turtlesim_node"
# Use inputs to cache the build task and only rerun it when files in src change
build = {cmd = "colcon build --symlink-install", inputs = ["src"]}
# A task that depends on the build task, so it will only run after a successful build
run   = {cmd = "ros2 run my_package my_node", depends-on = ["build"]}
```

```shell
pixi run sim
pixi run run
```
`run` runs both the `build` and `run` commands in sequence, but if you run it again without changing any files in `src`, it will skip the build step since the inputs haven't changed.

### Mixing ROS with Python ML libraries

```shell
# Tell pixi that it can install CUDA dependencies in this workspace
pixi workspace system-requirements add cuda 12

# (re)install ROS and your ML dependencies together
pixi add ros-humble-desktop pytorch torchvision opencv
```

---

## Community & Support

- [Pixi Discord](https://discord.gg/kKV8ZxyzY4): get help and share your projects
- [conda-forge](https://conda-forge.org/): the conda distribution Pixi is built on, with thousands of packages
- [RoboStack](https://robostack.github.io/): the conda channel that packages ROS for Pixi
- [Pixi GitHub](https://github.com/prefix-dev/pixi): report issues, contribute, and follow development

