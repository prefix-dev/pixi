[Jetbrains](https://www.jetbrains.com/ides/) has a suite of popular develop environments.

To use `pixi` inside of a Jetbrains IDE, follow these steps:

* Create a `pixi.toml` file.

See [`direnv`](../third_party/direnv.md).
* Install `direnv`.
* Add an `.envrc` to the project.

Use [`direnv`](../third_party/direnv.md) via a 
* Install the [Jetbrains `direnv` plugin](https://plugins.jetbrains.com/plugin/15285-direnv-integration)
which can be used to activate `pixi`.

Note: PyCharm belongs to the Jetbrains stable of IDE.
Unless you are doing polyglot projects it is recommended that the `pixi shell` approach,
[recommended here](pycharm.md) be used.
