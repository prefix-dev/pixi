One way to bring a Pixi package into production is to containerize it using tools like Docker or Podman.

<!-- Keep in sync with https://github.com/prefix-dev/pixi-docker/blob/main/README.md -->

We provide a simple docker image at [`pixi-docker`](https://github.com/prefix-dev/pixi-docker) that contains the Pixi executable on top of different base images.

The images are available on [ghcr.io/prefix-dev/pixi](https://ghcr.io/prefix-dev/pixi).

There are different tags for different base images available:

- `latest` - based on `ubuntu:jammy`
- `focal` - based on `ubuntu:focal`
- `bullseye` - based on `debian:bullseye`
- `noble-cuda-12.9.1` - based on `nvidia/cuda:12.9.1-base-ubuntu24.04`
- `noble-cuda-13.0.0` - based on `nvidia/cuda:13.0.0-base-ubuntu24.04`
- ... and more

!!!tip "All tags"
    For all tags, take a look at the [build script](https://github.com/prefix-dev/pixi-docker/blob/main/.github/workflows/build.yml).

!!!tip "Best practices for docker with pixi"
    [@pavelzw](https://github.com/pavelzw) wrote a blog post about [shipping conda environments to production using pixi](https://tech.quantco.com/blog/pixi-production).
    If you want to know more about best practices using docker with pixi, feel free to check out their blog post.

### Example Usage

The following example uses the Pixi docker image as a base image for a multi-stage build.
It also makes use of `pixi shell-hook` to not rely on Pixi being installed in the production container.

!!!tip "More examples"
    For more examples, take a look at [pavelzw/pixi-docker-example](https://github.com/pavelzw/pixi-docker-example).

```Dockerfile
FROM ghcr.io/prefix-dev/pixi:0.41.4 AS build

# copy source code, pixi.toml and pixi.lock to the container
WORKDIR /app
COPY . .
# install dependencies to `/app/.pixi/envs/prod`
# use `--locked` to ensure the lockfile is up to date with pixi.toml
RUN pixi install --locked -e prod
# create the shell-hook bash script to activate the environment
RUN pixi shell-hook -e prod -s bash > /shell-hook
RUN echo "#!/bin/bash" > /app/entrypoint.sh
RUN cat /shell-hook >> /app/entrypoint.sh
# extend the shell-hook script to run the command passed to the container
RUN echo 'exec "$@"' >> /app/entrypoint.sh

FROM ubuntu:24.04 AS production
WORKDIR /app
# only copy the production environment into prod container
# please note that the "prefix" (path) needs to stay the same as in the build container
COPY --from=build /app/.pixi/envs/prod /app/.pixi/envs/prod
COPY --from=build --chmod=0755 /app/entrypoint.sh /app/entrypoint.sh
# copy your project code into the container as well
COPY ./my_project /app/my_project

EXPOSE 8000
ENTRYPOINT [ "/app/entrypoint.sh" ]
# run your app inside the pixi environment
CMD [ "uvicorn", "my_project:app", "--host", "0.0.0.0" ]
```
