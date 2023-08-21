# Contributing to pixi
We would love for you to contribute to pixi and make it better with every PR merged!

To make contributing as easy as possible we made pixi a pixi project :wink:.

## Getting started
Clone our project locally on your machine:

```
git clone https://github.com/prefix-dev/pixi.git
```

Because pixi is a rust project we only need to use `cargo` but keeping it as simple as possible we also made some pixi tasks to help you get started.
Build the pixi with cargo by yourself.
```
pixi run build
```

After building cargo should have put a binary in the target folder that you can run from the command line.
```
./target/release/pixi
```

Run the tests

```
pixi run test
```

Installing your custom pixi into your machine can be done with.
```
pixi run install
```
**Note**: you might need to add the cargo env source to your shells configuration file

## Get your code ready for a PR
We use `pre-commit` to run all the formatters and linters we use.
If you have `pre-commit` on your system you can run `pre-commit install` to run the tools before you commit or push.
If you don't have it on your system either use `pixi global install pre-commit` or use the one in your environment.
```shell
pixi run lint
```

When you commit your code try to come up with a good commit message, we (try to) use [conventional-commits](https://www.conventionalcommits.org/en/v1.0.0/).
```shell
git add FILES_YOU_CHANGED
git commit -m "<type>[optional scope]: <description>"
git commit -m "feat: add xxx to the pixi.toml"
```
