FROM ghcr.io/prefix-dev/pixi:0.14.0 AS build

COPY . /app
WORKDIR /app
RUN pixi run build-wheel
RUN pixi run postinstall-production
RUN pixi shell-hook -e prod > /shell-hook
RUN echo "gunicorn -w 4 docker_project:app --bind :8000" >> /shell-hook

FROM ubuntu:22.04 AS production

# only copy the production environment into prod container
COPY --from=build /app/.pixi/envs/prod /app/.pixi/envs/prod
COPY --from=build /shell-hook /shell-hook
WORKDIR /app
EXPOSE 8000
CMD ["/bin/bash", "/shell-hook"]
