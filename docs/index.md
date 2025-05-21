**The next-generation package manager for reproducible, multi-language development.**

---

<div align="center" markdown="1">

[![Get Started](https://img.shields.io/badge/Get%20Started-Install%20Pixi-blue?style=flat-square)](#installation)
&nbsp;
[![GitHub stars](https://img.shields.io/github/stars/prefix-dev/pixi?style=flat-square&)](https://github.com/prefix-dev/pixi)
&nbsp;
![Discord](https://img.shields.io/discord/1082332781146800168?style=flat-square&logo=discord&logoColor=%23FFFFFF&color=%235865F2&link=https%3A%2F%2Fdiscord.gg%2FkKV8ZxyzY4)
&nbsp;
![License](https://img.shields.io/github/license/prefix-dev/pixi?style=flat-square&)

</div>


---

## 🚀 Why Pixi?

Pixi is a **fast, modern, and reproducible** package management tool for developers of all backgrounds.


| 🔄 **Reproducibility** | 🛠️ **Tasks** | 🌐 **Multi Platform** |
|---|---|---|
| Isolated, easily recreated environments with lockfiles built-in | Manage complex pipelines effortlessly. | Works on Linux, macOS, Windows, and more. |
| 🧩 **Multi Environment** | 🐍 **Python** | 🌍 **Global Tools** |
| Compose multiple environments in one manifest. | Support for `pyproject.toml` and PyPI through [`uv`](https://docs.astral.sh/uv/). | Install global tools, safely isolated. Replacing `apt`, `homebrew`, `winget`|

---


## ✨ Quick Demo

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
pixi global install git gh nvim ipython btop bat ripgrep
```
![Pixi Global Demo](assets/vhs-tapes/pixi_global_demo_light.gif#only-light)
![Pixi Global Demo](assets/vhs-tapes/pixi_global_demo_dark.gif#only-dark)

---


## ↔️ What is the difference with Pixi?

| Builtin Core Features | Pixi | Conda | Pip | Poetry | uv |
|-----------------------|---|---|---|---|---|
| Installs Python | ✅ | ✅ | ❌ | ❌ | ✅ |
| Supports more than Python | [✅]("Using the conda ecosystem Pixi installs any type of package, not just Python!") | ✅ | [❌]("Only Python packages") | [❌]("Only Python packages") |[❌]("Only Python packages") |
| Cross-platform Task Runner | [✅](workspace/advanced_tasks.md "Run shell commands on all platforms with `pixi run`") | ❌ | ❌ | ❌ | ✅ |
| Lockfiles | [✅](workspace/lockfile.md) | ❌ | ❌ | ✅ | ✅ |
| Project Management | [✅](reference/pixi_manifest.md) | ❌ | ❌ | ✅ | ✅ |

---


## 🛠️ Installation

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

## 🏁 Getting Started

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
---

## 💬 What Developers Say
> “I can’t stress enough how much I love using Pixi global as a package manager for my daily CLI tools. :)”

[*Matthew Feickert*](https://www.matthewfeickert.com/)


> "Pixi is the unifying dev experience that I've been wanting for robotics"

[*Audrow Nash*](https://x.com/audrow)

> “We are changing how we manage ROS dependencies on Windows.  We will be using Pixi to install and manage dependencies from conda. I'm pretty excited about how much easier it will be for users going forward.”

[*Michael Carroll*](https://x.com/carromj)

---

## 📚 Useful Links

- [GitHub](https://github.com/prefix-dev/pixi): Pixi source code, feel free to leave a star!
- [Discord](https://discord.gg/kKV8ZxyzY4): Join our community and ask questions.
- [Prefix.dev](https://prefix.dev/): The company behind Pixi, building the future of package management.
- [Conda-forge](https://conda-forge.org/): Community-driven collection of recipes for the conda package manager.
- [rattler](https://github.com/conda/rattler): Everything conda but built in Rust. Backend of Pixi.
- [rattler-build](https://rattler.build): A blazing fast build system for conda packages.

---
