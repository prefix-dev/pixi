# Contributing to pixi
We would love for you to contribute to pixi and make it better with every PR merged!

To make contributing as easy as possible we made pixi a pixi project :wink:.

## Getting started
Clone our project locally on your machine:

```
git clone https://github.com/prefix-dev/pixi.git
```

Because pixi is a [Rust](https://www.rust-lang.org/) project we only need to use `cargo` but keeping it as simple as possible we also made some pixi tasks to help you get started.
Build pixi with cargo by yourself.
```
pixi run build
```

After building, cargo should have placed a binary in the `target` folder that you can run from the command line.
```
./target/release/pixi
```

Run the tests:

```
pixi run test
```

Installing your custom built pixi binary into your machine can be done with:
```
pixi run install
```
**Note**: you might need to add the `cargo env source` to the configuration file of your shell e.g. `.bashrc`.

## Get your code ready for a PR
We use `pre-commit` to run all the formatters and linters that we use.
If you have `pre-commit` installed on your system you can run `pre-commit install` to run the tools before you commit or push.
If you don't have it on your system either use `pixi global install pre-commit` or use the one in your environment.
```shell
pixi run lint
```

When you commit your code, please try to come up with a good commit message.
The maintainers (try to) use [conventional-commits](https://www.conventionalcommits.org/en/v1.0.0/).
```shell
git add FILES_YOU_CHANGED
# This is the conventional commit convention:
git commit -m "<type>[optional scope]: <description>"
# An example:
git commit -m "feat: add xxx to the pixi.toml"
```
