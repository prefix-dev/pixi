# Bringing pixi to production

You can bring pixi projects into production by either containerizing it using tools like Docker or by using [`quantco/pixi-pack`](https://github.com/quantco/pixi-pack).

!!!tip ""
    [@pavelzw](https://github.com/pavelzw) from [QuantCo](https://quantco.com) wrote a blog post about bringing pixi to production. You can read it [here](https://tech.quantco.com/blog/pixi-production).

## Docker

<!-- Keep in sync with https://github.com/prefix-dev/pixi-docker/blob/main/README.md -->

We provide a simple docker image at [`pixi-docker`](https://github.com/prefix-dev/pixi-docker) that contains the pixi executable on top of different base images.

The images are available on [ghcr.io/prefix-dev/pixi](https://ghcr.io/prefix-dev/pixi).

There are different tags for different base images available:

- `latest` - based on `ubuntu:jammy`
- `focal` - based on `ubuntu:focal`
- `bullseye` - based on `debian:bullseye`
- `jammy-cuda-12.2.2` - based on `nvidia/cuda:12.2.2-jammy`
- ... and more

!!!tip "All tags"
    For all tags, take a look at the [build script](https://github.com/prefix-dev/pixi-docker/blob/main/.github/workflows/build.yml).

### Example usage

The following example uses the pixi docker image as a base image for a multi-stage build.
It also makes use of `pixi shell-hook` to not rely on pixi being installed in the production container.

!!!tip "More examples"
    For more examples, take a look at [pavelzw/pixi-docker-example](https://github.com/pavelzw/pixi-docker-example).

```Dockerfile
FROM ghcr.io/prefix-dev/pixi:0.40.0 AS build

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

## pixi-pack

<!-- Keep in sync with https://github.com/quantco/pixi-pack/blob/main/README.md -->

[`pixi-pack`](https://github.com/quantco/pixi-pack) is a simple tool that takes a pixi environment and packs it into a compressed archive that can be shipped to the target machine.

It can be installed via

```bash
pixi global install pixi-pack
```

Or by downloading our pre-built binaries from the [releases page](https://github.com/quantco/pixi-pack/releases).

Instead of installing pixi-pack globally, you can also use pixi exec to run `pixi-pack` in a temporary environment:

```bash
pixi exec pixi-pack pack
pixi exec pixi-pack unpack environment.tar
```

![pixi-pack demo](https://raw.githubusercontent.com/quantco/pixi-pack/refs/heads/main/.github/assets/demo/demo-light.gif#only-light)
![pixi-pack demo](https://raw.githubusercontent.com/quantco/pixi-pack/refs/heads/main/.github/assets/demo/demo-dark.gif#only-dark)

You can pack an environment with

```bash
pixi-pack pack --manifest-file pixi.toml --environment prod --platform linux-64
```

This will create a `environment.tar` file that contains all conda packages required to create the environment.

```plain
# environment.tar
| pixi-pack.json
| environment.yml
| channel
|    ├── noarch
|    |    ├── tzdata-2024a-h0c530f3_0.conda
|    |    ├── ...
|    |    └── repodata.json
|    └── linux-64
|         ├── ca-certificates-2024.2.2-hbcca054_0.conda
|         ├── ...
|         └── repodata.json
```

### Unpacking an environment

With `pixi-pack unpack environment.tar`, you can unpack the environment on your target system. This will create a new conda environment in `./env` that contains all packages specified in your `pixi.toml`. It also creates an `activate.sh` (or `activate.bat` on Windows) file that lets you activate the environment without needing to have `conda` or `micromamba` installed.

### Cross-platform packs

Since `pixi-pack` just downloads the `.conda` and `.tar.bz2` files from the conda repositories, you can trivially create packs for different platforms.

```bash
pixi-pack pack --platform win-64
```

!!!note ""
    You can only unpack a pack on a system that has the same platform as the pack was created for.

### Inject additional packages

You can inject additional packages into the environment that are not specified in `pixi.lock` by using the `--inject` flag:

```bash
pixi-pack pack --inject local-package-1.0.0-hbefa133_0.conda --manifest-pack pixi.toml
```

This can be particularly useful if you build the project itself and want to include the built package in the environment but still want to use `pixi.lock` from the project.

### Unpacking without pixi-pack

If you don't have `pixi-pack` available on your target system, you can still install the environment if you have `conda` or `micromamba` available.
Just unarchive the `environment.tar`, then you have a local channel on your system where all necessary packages are available.
Next to this local channel, you will find an `environment.yml` file that contains the environment specification.
You can then install the environment using `conda` or `micromamba`:

```bash
tar -xvf environment.tar
micromamba create -p ./env --file environment.yml
# or
conda env create -p ./env --file environment.yml
```

!!!note ""
    The `environment.yml` and `repodata.json` files are only for this use case, `pixi-pack unpack` does not use them.
