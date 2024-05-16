---
part: pixi
title: Frequently asked questions
description: What questions did we encounter more often?
---
## What is the difference with `conda`, `mamba`, `poetry`, `pip`

| Tool   | Installs python | Builds packages | Runs predefined tasks | Has lock files builtin | Fast | Use without python                                                     |
|--------|-----------------|-----------------|-----------------------|-----------------------|------|------------------------------------------------------------------------|
| Conda  | ✅               | ❌               | ❌                     | ❌                     | ❌    | ❌                                                                      |
| Mamba  | ✅               | ❌               | ❌                     | ❌                     | ✅    | [✅](https://mamba.readthedocs.io/en/latest/user_guide/micromamba.html) |
| Pip    | ❌               | ✅               | ❌                     | ❌                     | ❌    | ❌                                                                      |
| Pixi   | ✅               | 🚧              | ✅                     | ✅                     | ✅    | ✅                                                                      |
| Poetry | ❌               | ✅               | ❌                     | ✅                     | ❌    | ❌                                                                      |


## Why the name `pixi`
Starting with the name `prefix` we iterated until we had a name that was easy to pronounce, spell and remember.
There also wasn't a cli tool yet using that name.
Unlike `px`, `pex`, `pax`, etc.
We think it sparks curiosity and fun, if you don't agree, I'm sorry, but you can always alias it to whatever you like.

=== "Linux & macOS"
    ```shell
    alias not_pixi="pixi"
    ```
=== "Windows"
    PowerShell:
    ```powershell
    New-Alias -Name not_pixi -Value pixi
    ```

## Where is `pixi build`
**TL;DR**: It's coming we promise!

`pixi build` is going to be the subcommand that can generate a conda package out of a pixi project.
This requires a solid build tool which we're creating with [`rattler-build`](https://github.com/prefix-dev/rattler-build) which will be used as a library in pixi.
