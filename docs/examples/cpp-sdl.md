---
part: pixi/examples
title: SDL example
description: How to build and run an SDL application in C++
---

![](https://storage.googleapis.com/prefix-cms-images/docs/sdl_examle.png)

The `cpp-sdl` example is located in the pixi repository.

```shell
git clone https://github.com/prefix-dev/pixi.git
```

Move to the example folder

```shell
cd pixi/examples/cpp-sdl
```

Run the `start` command

```shell
pixi run start
```

Using the [`depends-on`](../features/advanced_tasks.md#depends-on) feature you only needed to run the `start` task but under water it is running the following tasks.

```shell
# Configure the CMake project
pixi run configure

# Build the executable
pixi run build

# Start the build executable
pixi run start
```
