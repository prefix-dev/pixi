# PAX: Package management made easy on ALL platforms

Pax is a universal package manager designed to make installing and managing dependencies in Python, C++, and Conda super easy.

Pax aims to provide AI/Data Science professionals the exceptional developer experience that is usually found with package managers like Cargo or Yarn.


# Features

- Seamless integration with Python, C++ and R using Conda packages
- All os's: linux, windows, osx and osx-arm
- A clean and simple cargo-like command-line interface.
- Project initialization and dependency management
- System-wide installation of conda packages

What is the main goal?

Who is it for?

Why would you use this over other managers?

# Installation
Install `pax`:
```bash
curl ....
```

Or, Build `pax` yourself:
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
Initialize the project
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
This behaviour is simular to `pipx` and `condax`.
```bash
pax install cowpy
```

# Contribution

We welcome contributions of all sorts. Even if you can't contribute code, reporting issues that you encounter is a great help.

# License

Pax is licensed under the .....
