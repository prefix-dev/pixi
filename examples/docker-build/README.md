# Build a docker image using pixi
This project shows how to build a docker image with pixi installed into it.

To show the strength of pixi in docker, we're going to use an installed pixi to build pixi in a docker image.
Steps of the docker build:
- Install the latest `pixi`.
- Install use `pixi` to install the build dependencies for `pixi`.
- Use `pixi` to run the cargo build of `pixi`.

NOTE: Please install docker manually as it is not available through conda.
To start the `docker build` use pixi:
```shell
pixi run start
```
