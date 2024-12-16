# Tutorial: Develop a ROS 2 package with `pixi`

In this tutorial, we will show you how to develop a ROS 2 package using `pixi`.
The tutorial is written to be executed from top to bottom, missing steps might result in errors.

The audience for this tutorial is developers who are familiar with ROS 2 and how are interested to try pixi for their development workflow.

## Prerequisites

- You need to have `pixi` installed. If you haven't installed it yet, you can follow the instructions in the [installation guide](../index.md).
  The crux of this tutorial is to show you only need pixi!
- On Windows, it's advised to enable Developer mode. Go to Settings -> Update & Security -> For developers -> Developer mode.

!!! note ""
    If you're new to pixi, you can check out the [basic usage](../basic_usage.md) guide.
    This will teach you the basics of pixi project within 3 minutes.

## Create a pixi project.

```shell
pixi init my_ros2_project -c robostack-staging -c conda-forge
cd my_ros2_project
```

It should have created a directory structure like this:

```shell
my_ros2_project
├── .gitattributes
├── .gitignore
└── pixi.toml
```

The `pixi.toml` file is the manifest file for your project. It should look like this:

```toml title="pixi.toml"
[project]
name = "my_ros2_project"
version = "0.1.0"
description = "Add a short description here"
authors = ["User Name <user.name@email.url>"]
channels = ["robostack-staging", "conda-forge"]
# Your project can support multiple platforms, the current platform will be automatically added.
platforms = ["linux-64"]

[tasks]

[dependencies]
```

The `channels` you added to the `init` command are repositories of packages, you can search in these repositories through our [prefix.dev](https://prefix.dev/channels) website.
The `platforms` are the systems you want to support, in pixi you can support multiple platforms, but you have to define which platforms, so pixi can test if those are supported for your dependencies.
For the rest of the fields, you can fill them in as you see fit.

## Add ROS 2 dependencies

To use a pixi project you don't need any dependencies on your system, all the dependencies you need should be added through pixi, so other users can use your project without any issues.

Let's start with the `turtlesim` example

```shell
pixi add ros-humble-desktop ros-humble-turtlesim
```

This will add the `ros-humble-desktop` and `ros-humble-turtlesim` packages to your manifest.
Depending on your internet speed this might take a minute, as it will also install ROS in your project folder (`.pixi`).

Now run the `turtlesim` example.

```shell
pixi run ros2 run turtlesim turtlesim_node
```

**Or** use the `shell` command to start an activated environment in your terminal.

```shell
pixi shell
ros2 run turtlesim turtlesim_node
```

Congratulations you have ROS 2 running on your machine with pixi!

??? example "Some more fun with the turtle"
    To control the turtle you can run the following command in a new terminal
    ```shell
    cd my_ros2_project
    pixi run ros2 run turtlesim turtle_teleop_key
    ```
    Now you can control the turtle with the arrow keys on your keyboard.

![Turtlesim control](https://github.com/user-attachments/assets/9424c44b-b7c0-48f4-8e7d-501131e9e9e5)

## Add a custom Python node

As ros works with custom nodes, let's add a custom node to our project.

```shell
pixi run ros2 pkg create --build-type ament_python --destination-directory src --node-name my_node my_package
```

To build the package we need some more dependencies:

```shell
pixi add colcon-common-extensions "setuptools<=58.2.0"
```

Add the created initialization script for the ros workspace to your manifest file.

Then run the build command

```shell
pixi run colcon build
```

This will create a sourceable script in the `install` folder, you can source this script through an activation script to use your custom node.
Normally this would be the script you add to your `.bashrc` but instead you tell pixi to use it by adding the following to `pixi.toml`:

=== "Linux & macOS"
    ```toml title="pixi.toml"
    [activation]
    scripts = ["install/setup.sh"]
    ```

=== "Windows"
    ```toml title="pixi.toml"
    [activation]
    scripts = ["install/setup.bat"]
    ```

??? tip "Multi platform support"
    You can add multiple activation scripts for different platforms, so you can support multiple platforms with one project.
    Use the following example to add support for both Linux and Windows, using the [target](../features/multi_platform_configuration.md#activation) syntax.

    ```toml
    [project]
    platforms = ["linux-64", "win-64"]

    [activation]
    scripts = ["install/setup.sh"]
    [target.win-64.activation]
    scripts = ["install/setup.bat"]
    ```

Now you can run your custom node with the following command

```shell
pixi run ros2 run my_package my_node
```

## Simplify the user experience

In `pixi` we have a feature called `tasks`, this allows you to define a task in your manifest file and run it with a simple command.
Let's add a task to run the `turtlesim` example and the custom node.

```shell
pixi task add sim "ros2 run turtlesim turtlesim_node"
pixi task add build "colcon build --symlink-install"
pixi task add hello "ros2 run my_package my_node"
```

Now you can run these task by simply running

```shell
pixi run sim
pixi run build
pixi run hello
```

???+ tip "Advanced task usage"
    Tasks are a powerful feature in pixi.

    - You can add [`depends-on`](../features/advanced_tasks.md#depends-on) to the tasks to create a task chain.
    - You can add [`cwd`](../features/advanced_tasks.md#working-directory) to the tasks to run the task in a different directory from the root of the project.
    - You can add [`inputs` and `outputs`](../features/advanced_tasks.md#caching) to the tasks to create a task that only runs when the inputs are changed.
    - You can use the [`target`](../reference/pixi_manifest.md#the-target-table) syntax to run specific tasks on specific machines.

```toml
[tasks]
sim = "ros2 run turtlesim turtlesim_node"
build = {cmd = "colcon build --symlink-install", inputs = ["src"]}
hello = { cmd = "ros2 run my_package my_node", depends-on = ["build"] }
```

## Build a C++ node

To build a C++ node you need to add the `ament_cmake` and some other build dependencies to your manifest file.

```shell
pixi add ros-humble-ament-cmake-auto compilers pkg-config cmake ninja
```

Now you can create a C++ node with the following command

```shell
pixi run ros2 pkg create --build-type ament_cmake --destination-directory src --node-name my_cpp_node my_cpp_package
```

Now you can build it again and run it with the following commands

```shell
# Passing arguments to the build command to build with Ninja, add them to the manifest if you want to default to ninja.
pixi run build --cmake-args -G Ninja
pixi run ros2 run my_cpp_package my_cpp_node
```

??? tip
    Add the cpp task to the manifest file to simplify the user experience.

    ```shell
    pixi task add hello-cpp "ros2 run my_cpp_package my_cpp_node"
    ```

## Conclusion
In this tutorial, we showed you how to create a Python & CMake ROS2 project using `pixi`.
We also showed you how to **add dependencies** to your project using `pixi`, and how to **run your project** using `pixi run`.
This way you can make sure that your project is **reproducible** on all your machines that have `pixi` installed.

## Show Off Your Work!
Finished with your project?
We'd love to see what you've created!
Share your work on social media using the hashtag #pixi and tag us @prefix_dev.
Let's inspire the community together!

## Frequently asked questions

### What happens with `rosdep`?

Currently, we don't support `rosdep` in a pixi environment, so you'll have to add the packages using `pixi add`.
`rosdep` will call `conda install` which isn't supported in a pixi environment.


### Community examples
ROS 2 Humble on macOS,[Simulating differential drive using Gazebo](https://medium.com/@davisogunsina/ros-2-macos-support-installing-and-running-ros-2-on-macos-79039d1d3655).