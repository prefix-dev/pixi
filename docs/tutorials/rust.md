# Tutorial: Develop a Rust package using `pixi`

In this tutorial, we will show you how to develop a Rust package using `pixi`.
The tutorial is written to be executed from top to bottom, missing steps might result in errors.

The audience for this tutorial is developers who are familiar with Rust and `cargo` and how are interested to try pixi for their development workflow.

!!! note ""
    If you're new to pixi, you can check out the [basic usage](../basic_usage.md) guide.
    This will teach you the basics of pixi project within 3 minutes.

## Prerequisites

- You need to have `pixi` installed. If you haven't installed it yet, you can follow the instructions in the [installation guide](../index.md).
  The crux of this tutorial is to show you only need pixi!

## Create a pixi project.

```shell
pixi init my_rust_project
cd my_rust_project
```

It should have created a directory structure like this:

```shell
my_rust_project
├── .gitattributes
├── .gitignore
└── pixi.toml
```

The `pixi.toml` file is the manifest file for your project. It should look like this:

```toml title="pixi.toml"
[project]
name = "my_rust_project"
version = "0.1.0"
description = "Add a short description here"
authors = ["User Name <user.name@email.url>"]
channels = ["conda-forge"]
platforms = ["linux-64"]

[tasks]

[dependencies]
```

## Add Rust dependencies

To use a pixi project you don't need any dependencies on your system, all the dependencies you need should be added through pixi, so other users can use your project without any issues.
```shell
pixi add rust
```

This will add the `rust` package to your `pixi.toml` file.

## Add a `cargo` project
As you now have rust installed, you can create a `cargo` project in your `pixi` project.
```shell
pixi run cargo init
```

Now we can build a `cargo` project using `pixi`.
```shell
pixi run cargo build
```
To simplify the build process, you can add a `build` task to your `pixi.toml` file using the following command:
```shell
pixi task add build "cargo build"
```
Which creates this field in the `pixi.toml` file:
```toml title="pixi.toml"
[tasks]
build = "cargo build"
```

And now you can build your project using:
```shell
pixi run build
```

You can also run your project using:
```shell
pixi run cargo run
```
Which you can simplify with a task again.
```shell
pixi task add start "cargo run"
```

So you should get the following output:
```shell
pixi run start
Hello, world!
```

Congratulations you have a Rust project running on your machine with pixi!

## Next steps, why is this useful when there is `rustup`?
Cargo is not a binary package manager, but a source package manager.
This means that you need to have the Rust compiler installed on your system to use it.
And possibly other dependencies that are not included in the `cargo` package manager.
For example, you might need to install `openssl` or `libssl-dev` on your system to build a package.
This is the case for `pixi` as well, but `pixi` will install these dependencies in your project folder, so you don't have to worry about them.

Add the following dependencies to your cargo project:
```shell
pixi run cargo add git2
```

If your system is not preconfigured to build C and have the `libssl-dev` package installed you will not be able to build the project:
```shell
pixi run build
...
Could not find directory of OpenSSL installation, and this `-sys` crate cannot
proceed without this knowledge. If OpenSSL is installed and this crate had
trouble finding it,  you can set the `OPENSSL_DIR` environment variable for the
compilation process.

Make sure you also have the development packages of openssl installed.
For example, `libssl-dev` on Ubuntu or `openssl-devel` on Fedora.

If you're in a situation where you think the directory *should* be found
automatically, please open a bug at https://github.com/sfackler/rust-openssl
and include information about your system as well as this message.

$HOST = x86_64-unknown-linux-gnu
$TARGET = x86_64-unknown-linux-gnu
openssl-sys = 0.9.102


It looks like you're compiling on Linux and also targeting Linux. Currently this
requires the `pkg-config` utility to find OpenSSL but unfortunately `pkg-config`
could not be found. If you have OpenSSL installed you can likely fix this by
installing `pkg-config`.
...
```
You can fix this with pixi:
```shell
pixi add openssl pkg-config compilers
```

Now you can build your project again:
```shell
pixi run build
...
   Compiling git2 v0.18.3
   Compiling my_rust_project v0.1.0 (/my_rust_project)
    Finished dev [unoptimized + debuginfo] target(s) in 7.44s
     Running `target/debug/my_rust_project`
```
