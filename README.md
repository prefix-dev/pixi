# PAX: Package management made easy
![License][license-badge]
[![Build Status][build-badge]][build]
[![Project Chat][chat-badge]][chat-url]

[license-badge]: https://img.shields.io/badge/license-BSD--3--Clause-blue?style=flat-square
[build-badge]: https://img.shields.io/github/actions/workflow/status/prefix-dev/pax/rust.yml?style=flat-square&branch=main
[build]: https://github.com/prefix-dev/pax/actions/
[chat-badge]: https://img.shields.io/discord/1082332781146800168.svg?label=&logo=discord&logoColor=ffffff&color=7389D8&labelColor=6A7EC2&style=flat-square
[chat-url]: https://discord.gg/kKV8ZxyzY4

`pax` is a universal package management tool designed to make installing and managing dependencies in Python, C++ and R using Conda packages.

`pax` aims to provide AI/Data Science professionals the exceptional developer experience that is usually found with package managers like `cargo` or `yarn`.

`pax` is completely written in Rust and build on top of the [rattler](https://github.com/mamba-org/rattler) library.

# Features

- Seamless integration with Python, C++ and R using Conda packages
- All os's: linux, windows, osx and osx-arm
- A clean and simple Cargo-like command-line interface.
- System-wide installation of Conda packages

# Installation
Install `pax`:
```bash
curl ....
```

Or, build `pax` yourself:
```bash
# Clone this project
git clone https://github.com/prefix-dev/pax.git

# Cargo install it to your system
cargo install --path pax
```

# Usage
The cli looks as follows:
```bash
➜ pax
Usage: pax <COMMAND>

Commands:
  completion  Generates a completion script for a shell
  init        Creates a new project
  add         Adds a dependency to the project
  run         Runs command in project
  help        Print this message or the help of the given subcommand(s)

Options:
  -h, --help  Print help

```
## Making a pax project
Initialize a new project
```
pax init myproject
```
Add the dependencies you want to use
```
cd myproject
pax add cowpy
```
Run the installed package in its environment
```bash
pax run cowpy Thanks for using pax
```

## Installing a conda package globally
Next to having a project linked the folder its in, you can also globally install conda packages into there own environment.
This behavior is similar to `pipx` and `condax`.
```bash
pax install cowpy
```

# Contribution 😍
We would absolutely love for you to contribute to `pax`!
Whether you want to start an issue, fix a bug you encountered, or suggest an improvement, every contribution is greatly appreciated.

If you're just getting started with our project or stepping into the Rust ecosystem for the first time, we've got your back!
We recommend beginning with issues labeled as `good first issue`.
These are carefully chosen tasks that provide a smooth entry point into contributing.These issues are typically more straightforward and are a great way to get familiar with the project.

Got questions or ideas, or just want to chat? Join our lively conversations on Discord.
We're very active and would be happy to welcome you to our community. [Join our discord server today!][chat-url]
