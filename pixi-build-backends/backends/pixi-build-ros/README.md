# ROS pixi backend
The is a PROTOTYPE pixi build backend for ROS packages.

This can be used from a path in a pixi package. It works if you add the following to your `pixi.toml`:

```toml
[package] 
name = "undefined" # Will be replaced by the value from the `package.xml`
version = "0.0.0" # Will be replaced by the value from the `package.xml`

[package.build]
backend = { name = "pixi-build-ros", path = "/absolute/path/to/pixi-build-backends/backends/pixi-build-ros" } 
configuration = { distro = "jazzy" }
```

# Interesting links used in development
- RoboStack stacks:
  - https://github.com/RoboStack/ros-noetic
  - https://github.com/RoboStack/ros-humble
  - https://github.com/RoboStack/ros-jazzy
- RoboStack Vinca: https://github.com/RoboStack/vinca
- How `rosdep` works: https://docs.ros.org/en/humble/Tutorials/Intermediate/Rosdep.html#how-does-rosdep-work

# Questions
- How to handle the [distribution yaml files](https://github.com/RoboStack/ros-humble/blob/main/robostack.yaml)?
  - Should we fetch them from the specific robostack repo?
  - How should users add to this on the go?
  - Should there be logic to handle the full mapping to `conda-forge`?
- How to deal with `conditions` in a `depend`? e.g.: `<exec_depend condition="$ROS_VERSION == 1">catkin</exec_depend>`, `<depend condition="$PLATFORM == X3">hobot-multimedia-dev</depend>`
- How do we handle `target` specific dependencies in a `package.xml`?

# Big TODOs
- [ ] Add the version of the dependencies to the requirements. They are fully ignored at the moment.
- [ ] Proper e2e tests.
- [ ] Add support for Windows.