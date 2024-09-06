FROM ghcr.io/prefix-dev/pixi:0.28.2 AS install

# Create a dummy blank project
WORKDIR /app/docker_project
RUN touch __init__.py

# Install dependencies
WORKDIR /app
COPY pyproject.toml .
COPY pixi.lock .
RUN --mount=type=cache,target=/root/.cache/rattler/cache,sharing=private pixi install

# Build for production
FROM install AS build

# Create an "entrypoint.sh" script which activates the pixi environment
RUN printf '#!/bin/sh\n%s\nexec "$@"' "$(pixi shell-hook -e prod)" > /entrypoint.sh
RUN chmod +x /entrypoint.sh

# Build then install the Python wheel
COPY . .
RUN pixi run build-wheel
RUN pixi run install-wheel

# Final minimal production image
FROM ubuntu:22.04 AS production

# only copy the production environment into prod container
COPY --from=build /app/.pixi/envs/prod /app/.pixi/envs/prod
COPY --from=build /entrypoint.sh /entrypoint.sh
WORKDIR /app
EXPOSE 8000
ENTRYPOINT ["/entrypoint.sh"]  # uses the pixi environment
CMD ["gunicorn", "docker_project:app", "-b", "0.0.0.0:8000"]
