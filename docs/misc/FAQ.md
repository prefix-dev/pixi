## What is the difference with `conda`, `mamba`, `poetry`, `pip`

| Tool   | Installs Python | Builds packages | Runs predefined tasks | Has lock files builtin | Fast | Use without python                                                     |
|--------|-----------------|-----------------|-----------------------|-----------------------|------|------------------------------------------------------------------------|
| Conda  | ✅               | ❌               | ❌                     | ❌                     | ❌    | ❌                                                                      |
| Mamba  | ✅               | ❌               | ❌                     | ❌                     | ✅    | [✅](https://mamba.readthedocs.io/en/latest/user_guide/micromamba.html) |
| Pip    | ❌               | ✅               | ❌                     | ❌                     | ❌    | ❌                                                                      |
| Pixi   | ✅               | 🚧              | ✅                     | ✅                     | ✅    | ✅                                                                      |
| Poetry | ❌               | ✅               | ❌                     | ✅                     | ❌    | ❌                                                                      |


## Why the name `pixi`
Starting with the name `prefix` we iterated until we had a name that was easy to pronounce, spell and remember.
There also wasn't a CLI tool yet using that name.
Unlike `px`, `pex`, `pax`, etc.
When in code mode we spell it like this `pixi`, otherwise we always start with an uppercase letter: Pixi.
We think the name sparks curiosity and fun, if you don't agree, I'm sorry, but you can always alias it to whatever you like.

=== "Linux & macOS"
    ```shell
    alias not_pixi="pixi"
    ```
=== "Windows"
    PowerShell:
    ```powershell
    New-Alias -Name not_pixi -Value pixi
    ```
