FROM ubuntu:latest

# Environment variables
ENV PIXI_VERSION=latest
ENV INSTALL_DIR=/usr/local/bin
ENV REPO=prefix-dev/pixi
ENV PLATFORM=unknown-linux-musl
ENV ARCH=x86_64
ENV PROJECT_NAME=pixi-in-docker

# Install curl and tar
RUN apt-get update && \
    apt-get install -y curl tar

# Download and install pixi
RUN if [ "$PIXI_VERSION" = "latest" ]; then \
      DOWNLOAD_URL="https://github.com/$REPO/releases/latest/download/pixi-$ARCH-$PLATFORM.tar.gz"; \
    else \
      DOWNLOAD_URL="https://github.com/$REPO/releases/download/$PIXI_VERSION/pixi-$ARCH-$PLATFORM.tar.gz"; \
    fi && \
    curl -SL "$DOWNLOAD_URL" | tar -xz -C "$INSTALL_DIR"

# Make a project dir and make it the workdir for the docker image.
RUN mkdir -p /root/$PROJECT_NAME
WORKDIR /root/$PROJECT_NAME

# Copy the project file and lockfile to only reinstall environment if those are changed.
COPY ../../pixi.lock ../../pixi.toml ./

# Install the environment
# The mount is a docker specific cache location so the firstime it will be slow but the second time the cache will be reused.
# More info in their docs: https://docs.docker.com/build/guide/mounts/
RUN --mount=type=cache,target=/root/.cache/rattler pixi install

# Copy the rest of the project
COPY ../../. ./

# Build pixi a custom pixi version in a docker container by running pixi the latest pixi ;)
RUN pixi run build
