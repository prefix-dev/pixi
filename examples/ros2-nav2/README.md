# Simple ROS2 NAV2 Example (Unsupported on Windows, but works in [WSL](https://learn.microsoft.com/en-us/windows/wsl/install))

The `start` task in PIXI runs the entire ROS2 launch command:
```shell
pixi run start
```
Executing this command will initiate the following:
- Gazebo: This physics simulator recreates the environment the robot navigates and collects sensor data.
- RViz2: As the ROS2 visualizer, it displays the robot's environment and allows control of the navigation.
- Navigation Stack: This comprises several elements, including a TurtleBot3 simulation, a map server, a controller server for managing requests, among other components.

Please note that the startup process may take a while.

![Gazebo & RViz with nav2](https://github.com/prefix-dev/pixi/assets/12893423/b33690d7-26d1-4304-903a-ff5020137832)

## Navigation Usage Instructions
- Ensure both RViz and Gazebo have successfully launched.
- In the RViz top bar, select `2D Pose Estimate`.
- Click and drag on the map to indicate the robot's approximate location. Accuracy is not crucial here; it's just a rough estimate.
- The map and odometry should now be connected.
- Choose `Nav2 Goal` from the top bar of RViz, then click on any location on the map to direct the robot there.
