---
title: Home
template: home.html
---

![pixi logo](assets/banner.svg)

---

<div align="center" markdown="1">

[![Get Started](https://img.shields.io/badge/Get%20Started-Install%20Pixi-blue?style=flat-square)](#installation)
&nbsp;
[![GitHub stars](https://img.shields.io/github/stars/prefix-dev/pixi?style=flat-square&)](https://github.com/prefix-dev/pixi)
&nbsp;
[![Discord](https://img.shields.io/discord/1082332781146800168?style=flat-square&logo=discord&logoColor=%23FFFFFF&color=%235865F2&link=https%3A%2F%2Fdiscord.gg%2FkKV8ZxyzY4)](https://discord.gg/kKV8ZxyzY4)
&nbsp;
[![License](https://img.shields.io/github/license/prefix-dev/pixi?style=flat-square&)](https://github.com/prefix-dev/pixi/blob/main/LICENSE)

</div>


---

## Why Pixi?

Pixi is a **fast, modern, and reproducible** package management tool for developers of all backgrounds.


| 🔄 **Reproducibility** | 🛠️ **Tasks** | 🌐 **Multi Platform** |
|---|---|---|
| Isolated, easily recreated environments with lockfiles built-in | Manage complex pipelines effortlessly. | Works on Linux, macOS, Windows, and more. |
| 🧩 **Multi Environment** | 🐍 **Python** | 🌍 **Global Tools** |
| Compose multiple environments in one manifest. | Support for `pyproject.toml` and PyPI through [`uv`](https://docs.astral.sh/uv/). | Install global tools, safely isolated. Replacing `apt`, `homebrew`, `winget`|

---


## Quick Demo

Project setup is a breeze with Pixi.
```shell
pixi init hello-world
cd hello-world
pixi add python
pixi run python -c "print('Hello World!')"
```
![Pixi Demo](assets/vhs-tapes/pixi_project_demo_light.gif#only-light)
![Pixi Demo](assets/vhs-tapes/pixi_project_demo_dark.gif#only-dark)

Install your favorite tools with a single command.
```shell
pixi global install gh nvim ipython btop ripgrep
```
![Pixi Global Demo](assets/vhs-tapes/pixi_global_demo_light.gif#only-light)
![Pixi Global Demo](assets/vhs-tapes/pixi_global_demo_dark.gif#only-dark)

---


## What is the difference with Pixi?

| Builtin Core Features | Pixi | Conda | Pip | Poetry | uv |
|-----------------------|---|---|---|---|---|
| Installs Python | ✅ | ✅ | ❌ | ❌ | ✅ |
| Supports more than Python | [✅]("Using the conda ecosystem Pixi installs any type of package, not just Python!") | ✅ | [❌]("Only Python packages") | [❌]("Only Python packages") |[❌]("Only Python packages") |
| Cross-platform Task Runner | [✅](workspace/advanced_tasks.md "Run shell commands on all platforms with `pixi run`") | ❌ | ❌ | ❌ | ✅ |
| Lockfiles | [✅](workspace/lockfile.md) | ❌ | ❌ | ✅ | ✅ |
| Project Management | [✅](reference/pixi_manifest.md) | ❌ | ❌ | ✅ | ✅ |

---

## Available software

Pixi installs and manages "conda" packages. We support the **biggest Conda package repository**, [conda-forge](https://conda-forge.org/), which contains over **30,000 packages** for Python, C/C++, Java, Rust, and more.
It is an open source, community-driven project, and you can add your own software as well (chat with us on [Discord](https://discord.gg/kKV8ZxyzY4) if you want to help!).

Some examples:

- **Python**: [`python`](https://prefix.dev/channels/conda-forge/packages/python), [`numpy`](https://prefix.dev/channels/conda-forge/packages/numpy), [`pandas`](https://prefix.dev/channels/conda-forge/packages/pandas), [`scikit-learn`](https://prefix.dev/channels/conda-forge/packages/scikit-learn), [`pytorch`](https://prefix.dev/channels/conda-forge/packages/pytorch)
- **C/C++**: [`clang`](https://prefix.dev/channels/conda-forge/packages/clang), [`boost`](https://prefix.dev/channels/conda-forge/packages/boost-cpp), [`gsl`](https://prefix.dev/channels/conda-forge/packages/gsl), [`eigen`](https://prefix.dev/channels/conda-forge/packages/eigen), [`fftw`](https://prefix.dev/channels/conda-forge/packages/fftw), [`hdf5`](https://prefix.dev/channels/conda-forge/packages/hdf5), [`opencv`](https://prefix.dev/channels/conda-forge/packages/opencv), [sdl2](https://prefix.dev/channels/conda-forge/packages/sdl2), [`cmake`](https://prefix.dev/channels/conda-forge/packages/cmake), [`meson`](https://prefix.dev/channels/conda-forge/packages/meson), [`ninja`](https://prefix.dev/channels/conda-forge/packages/ninja)
- **Java**: [`openjdk`](https://prefix.dev/channels/conda-forge/packages/openjdk), [`gradle`](https://prefix.dev/channels/conda-forge/packages/gradle), [`maven`](https://prefix.dev/channels/conda-forge/packages/maven)
- **Rust**: [`rust`](https://prefix.dev/channels/conda-forge/packages/rust), [`cargo-edit`](https://prefix.dev/channels/conda-forge/packages/cargo-edit), [`cargo-insta`](https://prefix.dev/channels/conda-forge/packages/cargo-insta), [`cargo-deny`](https://prefix.dev/channels/conda-forge/packages/cargo-deny)
- **Node.js**: [`nodejs`](https://prefix.dev/channels/conda-forge/packages/nodejs), [`pnpm`](https://prefix.dev/channels/conda-forge/packages/pnpm)
- **Cli Tools**: [`git`](https://prefix.dev/channels/conda-forge/packages/git), [`gh`](https://prefix.dev/channels/conda-forge/packages/gh), [`ripgrep`](https://prefix.dev/channels/conda-forge/packages/ripgrep), [`make`](https://prefix.dev/channels/conda-forge/packages/make)

And hundreds of other packages.

To browse the available packages, you can use the fast package search on [prefix.dev](https://prefix.dev/).


## Installation

To install `pixi`, run:

=== "Linux & macOS"
    ```bash
    curl -fsSL https://pixi.sh/install.sh | sh
    ```

=== "Windows"
    [Download installer](https://github.com/prefix-dev/pixi/releases/latest/download/pixi-x86_64-pc-windows-msvc.msi){ .md-button }

    Or run:

    ```powershell
    powershell -ExecutionPolicy ByPass -c "irm -useb https://pixi.sh/install.ps1 | iex"
    ```

!!! tip "Now restart your terminal or shell!"
    The installation needs to become effective by restarting your terminal or sourcing your shell.

??? question "Don't trust our link? Check the script!"
    You can check the installation `sh` script: [download](https://pixi.sh/install.sh) and the `ps1`: [download](https://pixi.sh/install.ps1).
    The scripts are open source and available on [GitHub](https://github.com/prefix-dev/pixi/tree/main/install).

[**See all installation options →**](installation.md)

---

## Getting Started

=== "Python"
    1. **Initialize a workspace:**
        ```
        pixi init hello-world
        cd hello-world
        ```

    2. **Add dependencies:**
        ```
        pixi add cowpy python
        ```

    3. **Create your script:**
        ```py title="hello.py"
        --8<-- "docs/source_files/pixi_workspaces/introduction/deps_add/hello.py"
        ```

    5. **Add a task:**
        ```
        pixi task add start python hello.py
        ```

    6. **Run the task:**
        ```
        pixi run start
        ```
        ```
        ✨ Pixi task (start): python hello.py
        __________________
        < Hello Pixi fans! >
        ------------------
            \   ^__^
            \  (oo)\_______
                (__)\       )\/\
                ||----w |
                ||     ||
        ```

    7. **Entry the environment shell:**
        ```
        pixi shell
        python hello.py
        exit
        ```

    More details on how to use Pixi with Python can be found in the [Python tutorial](python/tutorial.md).

=== "Rust"
    1. **Initialize a workspace:**
        ```
        pixi init pixi-rust
        cd pixi-rust
        ```
    2. **Add dependencies:**
        ```
        pixi add rust
        ```
    3. **Create your script:**
        ```rust title="hello.rs"
        fn main() {
            println!("Hello Pixi fans!");
        }
        ```
    4. **Add a task:**
        ```
        pixi task add build "rustc hello.rs"
        ```
    5. **Run the task:**
        ```
        pixi run build
        ```
    6. **Run the script:**
        ```
        ./hello
        ```
        ```
        Hello Pixi fans!
        ```

    This is more of an example to show off how easy it is to use Pixi with Rust.
    Not a recommended way to build Rust projects.
    More details on how to use Pixi with Rust can be found in the [Rust tutorial](tutorials/rust.md).

=== "Node.js"
    1. **Initialize a workspace:**
        ```
        pixi init pixi-node
        cd pixi-node
        ```
    2. **Add dependencies:**
        ```
        pixi add nodejs
        ```
    3. **Create your script:**
        ```js title="hello.js"
        console.log("Hello Pixi fans!");
        ```
    4. **Add a task:**
        ```
        pixi task add start "node hello.js"
        ```
    5. **Run the task:**
        ```
        pixi run start
        ```
        ```
        ✨ Pixi task (start): node hello.js
        Hello Pixi fans!
        ```

=== "ROS2"

    1. **Initialize a workspace:**
        ```
        pixi init pixi-ros2 -c https://prefix.dev/conda-forge -c "https://prefix.dev/robostack-humble"
        cd pixi-ros2
        ```
    2. **Add dependencies:**
        ```
        pixi add ros-humble-desktop
        ```

        ??? tip "This might take a minute"
            Depending on your internet connection, this will take a while to install, as it will download the entire ROS2 desktop package.

    3. **Start Rviz**
        ```
        pixi run rviz2
        ```

    More details on how to use Pixi with ROS2 can be found in the [ROS2 tutorial](tutorials/ros2.md).
=== "DevOps"
    1. Install all your favorite tools with a single command:
    ```shell
    pixi global install terraform ansible k9s make
    ```
    2. Use them everywhere:
    ```shell
    ansible --version
    terraform --version
    k9s version
    make --version
    ```


---

## What Developers Say

<div align="center" markdown="1">

_**“I can’t stress enough how much I love using Pixi global as a package manager for my daily CLI tools. :)”**_

*Matthew Feickert* [University of Wisconsin–Madison](https://www.wisc.edu/)


_**“We are changing how we manage ROS dependencies on Windows.  We will be using Pixi to install and manage dependencies from conda. I'm pretty excited about how much easier it will be for users going forward.”**_

*Michael Carroll* [Project Lead ROS](https://www.ros.org/)

</div>

---

## Useful Links

- [GitHub](https://github.com/prefix-dev/pixi): Pixi source code, feel free to leave a star!
- [Discord](https://discord.gg/kKV8ZxyzY4): Join our community and ask questions.
- [Prefix.dev](https://prefix.dev/): The company behind Pixi, building the future of package management.
- [Conda-forge](https://conda-forge.org/): Community-driven collection of recipes for the conda package manager.
- [rattler](https://github.com/conda/rattler): Everything conda but built in Rust. Backend of Pixi.
- [rattler-build](https://rattler.build): A blazing fast build system for conda packages.

---
