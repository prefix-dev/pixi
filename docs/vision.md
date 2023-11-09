---
part: pixi
title: Pixi Vision
description: What is the vision for pixi?
---

# Vision

We created `pixi` because we want to have a cargo/npm/yarn like package management experience for conda. We really love what the conda packaging ecosystem achieves, but we think that the user experience can be improved a lot.
Modern package managers like `cargo` have shown us, how great a package manager can be. We want to bring that experience to the conda ecosystem.

## Pixi values

We want to make pixi a great experience for everyone, so we have a few values that we want to uphold:

1. **Fast**. We want to have a fast package manager, that is able to solve the environment in a few seconds.
2. **User Friendly**. We want to have a package manager that puts user friendliness on the front-line. Providing easy, accessible and intuitive commands. That have the element of _least surprise_.
3. **Isolated Environment**. We want to have isolated environments, that are reproducible and easy to share. Ideally, it should run on all common platforms. The Conda packaging system provides an excellent base for this.
4. **Single Tool**. We want to integrate most common uses when working on a development project with Pixi, so it should support at least dependency management, command management, building and uploading packages. You should not need to reach to another external tool for this.
5. **Fun**. It should be fun to use pixi and not cause frustrations, you should not need to think about it a lot and it should generally just get out of your way.

## Conda

We are building on top of the conda packaging ecosystem, this means that we have a huge number of packages available for different platforms on [conda-forge](https://conda-forge.org/). We believe the conda packaging ecosystem provides a solid base to manage your dependencies. Conda-forge is community maintained and very open to contributions. It is widely used in data science and scientific computing, robotics and other fields. And has a proven track record.

## Target languages

Essentially, we are language agnostics, we are targeting any language that can be installed with conda. Including: C++, Python, Rust, Zig etc.
But we do believe the python ecosystem can benefit from a good package manager that is based on conda.
So we are trying to provide an alternative to existing solutions there.
We also think we can provide a good solution for C++ projects, as there are a lot of libraries available on conda-forge today.
Pixi also truly shines when using it for multi-language projects e.g. a mix of C++ and Python, because we provide a nice way to build everything up to and including
system level packages.
