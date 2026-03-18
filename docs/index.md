---
title: Home
template: home.html
---
# Pixi

Pixi is a **fast** workspace manager. Beyond installing packages, Pixi manages reproducible
development environments with [lockfiles](workspace/lockfile.md), runs
[tasks](workspace/advanced_tasks.md) (like build scripts, test runners, or linters), supports
[multiple environments](workspace/multi_environment.md) in a single workspace (e.g. separate `test`
and `docs` environments), and works across
[Linux, macOS, and Windows](workspace/multi_platform_configuration.md) from the same declaration.

By building on conda, Pixi gives you access to tens of thousands of ready-to-use packages from
[conda-forge](https://conda-forge.org/) — covering everything from Python and R to C++ libraries,
compilers, and CLI tools. Beyond conda-forge, specialized channels serve specific communities:
[robostack](https://robostack.github.io/) for OS-independent ROS packages or
[bioconda](https://bioconda.github.io/) for bioinformatics spring to mind.

The ecosystem also works both ways: when you
[build your own packages](build/getting_started.md) with [pixi build](build/getting_started.md),
you can publish them to a channel and make them installable by anyone using Pixi, or any other
conda-based tooling — tying your work into a distribution network that already serves millions of
users. [prefix.dev](https://prefix.dev/channels/) makes it easy to host both public and private
channels, so you can share packages with the world or keep them internal to your team.

---

## Quick Demo

Project setup is a breeze with Pixi.

```shell
pixi init hello-world
cd hello-world
pixi add python
pixi run python -c 'print("Hello World!")'
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

## How Tools Compare to Pixi

| Builtin Core Features       | Pixi | Conda | Pip | Poetry | uv |
|-----------------------------|------|-------|-----|--------|----|
| Installs Python             | ✅    | ✅     | ❌   | ❌      | ✅  |
| Supports Multiple Languages | ✅    | ✅     | ❌   | ❌      | ❌  |
| Lockfiles                   | ✅    | ❌     | ❌   | ✅      | ✅  |
| Task runner                 | ✅    | ❌     | ❌   | ❌      | ❌  |
| Workspace Management        | ✅    | ❌     | ❌   | ✅      | ✅  |

---

## Available Software

Pixi defaults to the **biggest Conda package repository**, [conda-forge](https://conda-forge.org/), which contains over
**30,000 packages**.

- **Python**: [`python`](https://prefix.dev/channels/conda-forge/packages/python), [`scikit-learn`](https://prefix.dev/channels/conda-forge/packages/scikit-learn), [`pytorch`](https://prefix.dev/channels/conda-forge/packages/pytorch)
- **C/C++**: [`clang`](https://prefix.dev/channels/conda-forge/packages/clang), [`boost`](https://prefix.dev/channels/conda-forge/packages/libboost-devel), [`opencv`](https://prefix.dev/channels/conda-forge/packages/opencv), [`ninja`](https://prefix.dev/channels/conda-forge/packages/ninja)
- **Java**: [`openjdk`](https://prefix.dev/channels/conda-forge/packages/openjdk), [`gradle`](https://prefix.dev/channels/conda-forge/packages/gradle), [`maven`](https://prefix.dev/channels/conda-forge/packages/maven)
- **Rust**: [`rust`](https://prefix.dev/channels/conda-forge/packages/rust), [`cargo-edit`](https://prefix.dev/channels/conda-forge/packages/cargo-edit), [`cargo-insta`](https://prefix.dev/channels/conda-forge/packages/cargo-insta)
- **Node.js**: [`nodejs`](https://prefix.dev/channels/conda-forge/packages/nodejs), [`bun`](https://prefix.dev/channels/conda-forge/packages/bun), [`pnpm`](https://prefix.dev/channels/conda-forge/packages/pnpm), [`eslint`](https://prefix.dev/channels/conda-forge/packages/eslint)
- **Cli Tools**: [`git`](https://prefix.dev/channels/conda-forge/packages/git), [`gh`](https://prefix.dev/channels/conda-forge/packages/gh), [`ripgrep`](https://prefix.dev/channels/conda-forge/packages/ripgrep), [`make`](https://prefix.dev/channels/conda-forge/packages/make)

And browse the thousands more on [prefix.dev](https://prefix.dev/), or host [your own channels](https://prefix.dev/channels/)

---

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
    2. **Add dependencies into the default environment:**
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
    7. **Enter the shell into the environment:**
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
    3. **Create your workspace:**
        ```
        pixi run cargo init
        ```
    4. **Add a task:**
        ```
        pixi task add start cargo run
        ```
    5. **Run the task:**
        ```
        pixi run start
        ```
        ```
        ✨ Pixi task (start): cargo run
            Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.02s
             Running `target/debug/pixi-rust`
        Hello, world!
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

<div class="quote-scroll-wrapper">
  <div class="quote-scroll">
    <div class="quote-card">
      <p>"Pixi is my tool of choice for Python environment management. It has significantly reduced boilerplate by offering seamless support for both PyPI and conda-forge indexes - a critical requirement in my workflow."</p>
      <strong>Guillaume Lemaitre</strong> – <a href="https://scikit-learn.org">scikit-learn</a>
    </div>
    <div class="quote-card">
      <p>"I can’t stress enough how much I love using Pixi global as a package manager for my daily CLI tools. With the global manifest, even sharing my setup across machines is trivial!"</p>
      <strong>Matthew Feickert</strong> – <a href="https://www.wisc.edu/">University of Wisconsin–Madison</a>
    </div>
    <div class="quote-card">
      <p>"We are changing how we manage ROS dependencies on Windows. We will be using Pixi to install and manage dependencies from conda. I'm pretty excited about how much easier it will be for users going forward."</p>
      <strong>Michael Carroll</strong> – <a href="https://www.ros.org/">Project Lead ROS</a>
    </div>
  </div>
</div>

---

## Useful Links

- [GitHub](https://github.com/prefix-dev/pixi): Pixi source code, feel free to leave a star!
- [Discord](https://discord.gg/kKV8ZxyzY4): Join our community and ask questions.
- [Prefix.dev](https://prefix.dev/): The company behind Pixi, building the future of package management.
- [conda-forge](https://conda-forge.org/): Community-driven collection of recipes for the conda package manager.
- [Rattler](https://github.com/conda/rattler): Everything conda but built in Rust. Backend of Pixi.
- [rattler-build](https://rattler.build): A blazing fast build system for conda packages.
