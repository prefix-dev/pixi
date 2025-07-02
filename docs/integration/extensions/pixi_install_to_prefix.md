Pixi installs your environments to `.pixi/envs/<env-name>` by default.
If you want to install your environment to an arbitrary location on your system, you can use [`pixi-install-to-prefix`](https://github.com/pavelzw/pixi-install-to-prefix).

You can install `pixi-install-to-prefix` with:

```bash
pixi global install pixi-install-to-prefix
```

Instead of installing `pixi-install-to-prefix` globally, you can also use `pixi exec` to run `pixi-install-to-prefix` in a temporary environment:

```bash
pixi exec pixi-install-to-prefix ./my-environment
```

```text
Usage: pixi-install-to-prefix [OPTIONS] <PREFIX>

Arguments:
  <PREFIX>  The path to the prefix where you want to install the environment

Options:
  -l, --lockfile <LOCKFILE>        The path to the pixi lockfile [default: pixi.lock]
  -e, --environment <ENVIRONMENT>  The name of the pixi environment to install [default: default]
  -p, --platform <PLATFORM>        The platform you want to install for [default: <your-system-platform>]
  -c, --config <CONFIG>            The path to the pixi config file. By default, no config file is used
  -s, --shell <SHELL>              The shell(s) to generate activation scripts for. Default: see README
      --no-activation-scripts      Disable the generation of activation scripts
  -v, --verbose...                 Increase logging verbosity
  -q, --quiet...                   Decrease logging verbosity
  -h, --help                       Print help
```
