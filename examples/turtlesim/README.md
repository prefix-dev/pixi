# Simple ROS turtlesim example

## To run the ROS2 humble example
Run this in **two** terminals.
```shell
pixi run start
```
Select the `default` environment for all of the `humble` examples.

And then
```shell
pixi run teleop
```

It is also possible to start a shell in the environment.
There you can run any of the commands available in the environment.
```shell
pixi shell
ros2 topic echo /turtle1/cmd_vel
```

### Now run a script to visualize the turtle in rviz
Run this in **two** more terminals.
```shell
pixi run rviz
```

```shell
pixi run viz
```

## To run the ROS1 Noetic example

Run this in **three** terminals.
```shell
# Start roscore
pixi run -e noetic core
```
or without `-e noetic` and select the `noetic` environment for all of the `noetic` examples.

```shell
# Start turtlesim
pixi run -e noetic start
```

```shell
pixi run -e noetic teleop
```

It is also possible to start a shell in the environment.
There you can run any of the commands available in the environment.
```shell
pixi shell -e noetic
rostopic echo /turtle1/cmd_vel
```

### Now run a script to visualize the turtle in rviz
Run this in **two** more terminals.
```shell
pixi run -e noetic rviz
```

```shell
pixi run -e noetic viz
```

Add the visual marker for the turtle to the displays in rviz.

If you now teleop the turtle, you should see it move in rviz aswell now.
