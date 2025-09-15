## Why Pixi?

Pixi is a **fast, modern, and reproducible** package management tool for developers of all backgrounds.

\[**🔄 Reproducibility**

Isolated, easily recreated environments with lockfiles built-in\](workspace/lockfile) \[**🛠️ Tasks**

Manage complex pipelines effortlessly.\](workspace/advanced_tasks/) \[**🌐 Multi Platform**

Works on Linux, macOS, Windows, and more.\](workspace/multi_platform_configuration/) \[**🧩 Multi Environment**

Compose multiple environments in one manifest.\](workspace/multi_environment/) \[**🐍 Python**

Support for `pyproject.toml` and PyPI through uv.\](python/tutorial/) \[**🌍 Global Tools**

Install global tools, safely isolated. Replacing `apt`, `homebrew`, `winget`\](global_tools/introduction/)

______________________________________________________________________

## Quick Demo

Project setup is a breeze with Pixi.

```shell
pixi init hello-world
cd hello-world
pixi add python
pixi run python -c 'print("Hello World!")'

```

Install your favorite tools with a single command.

```shell
pixi global install gh nvim ipython btop ripgrep

```

______________________________________________________________________

## What is the difference with Pixi?

| Builtin Core Features       | Pixi | Conda | Pip | Poetry | uv  |
| --------------------------- | ---- | ----- | --- | ------ | --- |
| Installs Python             | ✅   | ✅    | ❌  | ❌     | ✅  |
| Supports Multiple Languages | ✅   | ✅    | ❌  | ❌     | ❌  |
| Lockfiles                   | ✅   | ❌    | ❌  | ✅     | ✅  |
| Task runner                 | ✅   | ❌    | ❌  | ❌     | ❌  |
| Workspace Management        | ✅   | ❌    | ❌  | ✅     | ✅  |

______________________________________________________________________

## Available Software

Pixi defaults to the **biggest Conda package repository**, [conda-forge](https://conda-forge.org/), which contains over **30,000 packages**.

- **Python**: [`python`](https://prefix.dev/channels/conda-forge/packages/python), [`scikit-learn`](https://prefix.dev/channels/conda-forge/packages/scikit-learn), [`pytorch`](https://prefix.dev/channels/conda-forge/packages/pytorch)
- **C/C++**: [`clang`](https://prefix.dev/channels/conda-forge/packages/clang), [`boost`](https://prefix.dev/channels/conda-forge/packages/boost-cpp), [`opencv`](https://prefix.dev/channels/conda-forge/packages/opencv), [`ninja`](https://prefix.dev/channels/conda-forge/packages/ninja)
- **Java**: [`openjdk`](https://prefix.dev/channels/conda-forge/packages/openjdk), [`gradle`](https://prefix.dev/channels/conda-forge/packages/gradle), [`maven`](https://prefix.dev/channels/conda-forge/packages/maven)
- **Rust**: [`rust`](https://prefix.dev/channels/conda-forge/packages/rust), [`cargo-edit`](https://prefix.dev/channels/conda-forge/packages/cargo-edit), [`cargo-insta`](https://prefix.dev/channels/conda-forge/packages/cargo-insta)
- **Node.js**: [`nodejs`](https://prefix.dev/channels/conda-forge/packages/nodejs), [`pnpm`](https://prefix.dev/channels/conda-forge/packages/pnpm), [`eslint`](https://prefix.dev/channels/conda-forge/packages/eslint)
- **Cli Tools**: [`git`](https://prefix.dev/channels/conda-forge/packages/git), [`gh`](https://prefix.dev/channels/conda-forge/packages/gh), [`ripgrep`](https://prefix.dev/channels/conda-forge/packages/ripgrep), [`make`](https://prefix.dev/channels/conda-forge/packages/make)

And browse the thousands more on [prefix.dev](https://prefix.dev/), or host [your own channels](https://prefix.dev/channels/)

______________________________________________________________________

## Installation

To install `pixi`, run:

```bash
curl -fsSL https://pixi.sh/install.sh | sh

```

[Download installer](https://github.com/prefix-dev/pixi/releases/latest/download/pixi-x86_64-pc-windows-msvc.msi)

Or run:

```powershell
powershell -ExecutionPolicy ByPass -c "irm -useb https://pixi.sh/install.ps1 | iex"

```

Now restart your terminal or shell!

The installation needs to become effective by restarting your terminal or sourcing your shell.

Don't trust our link? Check the script!

You can check the installation `sh` script: [download](https://pixi.sh/install.sh) and the `ps1`: [download](https://pixi.sh/install.ps1). The scripts are open source and available on [GitHub](https://github.com/prefix-dev/pixi/tree/main/install).

[**See all installation options →**](installation/)

______________________________________________________________________

## Getting Started

1. **Initialize a workspace:**

   ```text
   pixi init hello-world
   cd hello-world

   ```

1. **Add dependencies:**

   ```text
   pixi add cowpy python

   ```

1. **Create your script:** hello.py

   ```py
   from cowpy.cow import Cowacter
   message = Cowacter().milk("Hello Pixi fans!")
   print(message)

   ```

1. **Add a task:**

   ```text
   pixi task add start python hello.py

   ```

1. **Run the task:**

   ```text
   pixi run start

   ```

   ```text
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

1. **Entry the environment shell:**

   ```text
   pixi shell
   python hello.py
   exit

   ```

More details on how to use Pixi with Python can be found in the [Python tutorial](python/tutorial/).

1. **Initialize a workspace:** `pixi init pixi-rust cd pixi-rust`

1. **Add dependencies:**

   ```text
   pixi add rust

   ```

1. **Create your workspace:**

   ```text
   pixi run cargo init

   ```

1. **Add a task:**

   ```text
   pixi task add start cargo run

   ```

1. **Run the task:**

   ```text
   pixi run start

   ```

   ```text
   ✨ Pixi task (start): cargo run
       Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.02s
        Running `target/debug/pixi-rust`
   Hello, world!

   ```

This is more of an example to show off how easy it is to use Pixi with Rust. Not a recommended way to build Rust projects. More details on how to use Pixi with Rust can be found in the [Rust tutorial](tutorials/rust/).

1. **Initialize a workspace:**

   ```text
   pixi init pixi-node
   cd pixi-node

   ```

1. **Add dependencies:**

   ```text
   pixi add nodejs

   ```

1. **Create your script:** hello.js

   ```js
   console.log("Hello Pixi fans!");

   ```

1. **Add a task:**

   ```text
   pixi task add start "node hello.js"

   ```

1. **Run the task:**

   ```text
   pixi run start

   ```

   ```text
   ✨ Pixi task (start): node hello.js
   Hello Pixi fans!

   ```

1. **Initialize a workspace:**

   ```text
   pixi init pixi-ros2 -c https://prefix.dev/conda-forge -c "https://prefix.dev/robostack-humble"
   cd pixi-ros2

   ```

1. **Add dependencies:**

   ```text
   pixi add ros-humble-desktop

   ```

   This might take a minute

   Depending on your internet connection, this will take a while to install, as it will download the entire ROS2 desktop package.

1. **Start Rviz**

   ```text
   pixi run rviz2

   ```

More details on how to use Pixi with ROS2 can be found in the [ROS2 tutorial](tutorials/ros2/).

1. Install all your favorite tools with a single command:
   ```shell
   pixi global install terraform ansible k9s make

   ```
1. Use them everywhere:
   ```shell
   ansible --version
   terraform --version
   k9s version
   make --version

   ```

______________________________________________________________________

## What Developers Say

"Pixi is my tool of choice for Python environment management. It has significantly reduced boilerplate by offering seamless support for both PyPI and conda-forge indexes - a critical requirement in my workflow."

**Guillaume Lemaitre** – [scikit-learn](https://scikit-learn.org)

"I can’t stress enough how much I love using Pixi global as a package manager for my daily CLI tools. With the global manifest, even sharing my setup across machines is trivial!"

**Matthew Feickert** – [University of Wisconsin–Madison](https://www.wisc.edu/)

"We are changing how we manage ROS dependencies on Windows. We will be using Pixi to install and manage dependencies from conda. I'm pretty excited about how much easier it will be for users going forward."

**Michael Carroll** – [Project Lead ROS](https://www.ros.org/)

______________________________________________________________________

## Useful Links

- [GitHub](https://github.com/prefix-dev/pixi): Pixi source code, feel free to leave a star!
- [Discord](https://discord.gg/kKV8ZxyzY4): Join our community and ask questions.
- [Prefix.dev](https://prefix.dev/): The company behind Pixi, building the future of package management.
- [conda-forge](https://conda-forge.org/): Community-driven collection of recipes for the conda package manager.
- [Rattler](https://github.com/conda/rattler): Everything conda but built in Rust. Backend of Pixi.
- [rattler-build](https://rattler.build): A blazing fast build system for conda packages.
