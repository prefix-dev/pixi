---
part: pixi
title: Environment
description: The resulting environment of a pixi installation.
---

# Environment variables
The following environment variables are set by pixi, when using the `pixi run`, `pixi shell`, or `pixi shell-hook` command:

- `PIXI_PROJECT_ROOT`: The root directory of the project.
- `PIXI_PROJECT_NAME`: The name of the project.
- `PIXI_PROJECT_MANIFEST`: The path to the manifest file (`pixi.toml`).
- `PIXI_PROJECT_VERSION`: The version of the project.
- `PIXI_PROMPT`: The prompt to use in the shell, also used by `pixi shell` itself.
- `PIXI_ENVIRONMENT_NAME`: The name of the environment, defaults to `default`.
- `PIXI_ENVIRONMENT_PLATFORMS`: The path to the environment.
- `CONDA_PREFIX`: The path to the environment. (Used by multiple tools that already understand conda environments)
- `CONDA_DEFAULT_ENV`: The name of the environment. (Used by multiple tools that already understand conda environments)
- `PATH`: We prepend the `bin` directory of the environment to the `PATH` variable, so you can use the tools installed in the environment directly.
