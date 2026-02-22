# docker

This example demonstrates using docker in combination with [solve-groups](https://pixi.sh/latest/configuration/#the-environments-table) and per-environment editability.

## Per-environment editability

The project uses solve-groups to ensure that the `default` environment (for development and testing) and the `prod` environment use *exactly* the same versions of dependencies. Both environments include the local `docker-project` package, but with different editability:

- **`default` environment**: The package is installed as editable (`editable = true`) for fast development iteration
- **`prod` environment**: The package is installed as non-editable (`editable = false`) for a clean production deployment

This is configured in `pyproject.toml`:
```toml
[tool.pixi.feature.dev.pypi-dependencies]
docker-project = { path = ".", editable = true }

[tool.pixi.feature.prod.pypi-dependencies]
docker-project = { path = ".", editable = false }

[tool.pixi.environments]
default = { features = ["test", "dev"], solve-group = "default" }
prod = { features = ["prod"], solve-group = "default" }
```

## Docker setup

In the docker container, we only copy the `prod` environment into the final layer, so the `default` environment and all its dependencies are not included in the final image.
Also, `pixi` itself is not included in the final image and we activate the environment using `pixi -e prod shell-hook`.

## Usage

To build and run the docker container you require [`docker`](https://docs.docker.com/engine/install/) or [`podman`](https://podman.io) and [`docker-compose`](https://docs.docker.com/compose/install/).

### Run a development server

```shell
docker compose up --build
```

### Build for production and run

```shell
docker build -t pixi-docker .
docker run -p 8000:8000 pixi-docker
```
