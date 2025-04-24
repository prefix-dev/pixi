
## Configurable Environment Variables

Pixi can also be configured via environment variables.

<table>
  <thead>
    <tr>
      <th>Name</th>
      <th>Description</th>
      <th>Default</th>
    </tr>
  </thead>
  <tbody>
    <tr>
      <td><code>PIXI_HOME</code></td>
      <td>Defines the directory where pixi puts its global data.</td>
      <td><a href="https://docs.rs/dirs/latest/dirs/fn.home_dir.html">HOME</a>/.pixi</td>
    </tr>
    <tr>
      <td><code>PIXI_CACHE_DIR</code></td>
      <td>Defines the directory where pixi puts its cache.</td>
      <td>
        <ul>
          <li>If <code>PIXI_CACHE_DIR</code> is not set, the <code>RATTLER_CACHE_DIR</code> environment variable is used.</li>
          <li>If that is not set, <code>XDG_CACHE_HOME/pixi</code> is used when the directory exists.</li>
          <li>If that is not set, the default cache directory of <a href="https://docs.rs/rattler/latest/rattler/fn.default_cache_dir.html">rattler::default_cache_dir</a> is used.</li>
        </ul>
      </td>
    </tr>
  </tbody>
</table>


## Environment Variables Set By Pixi

The following environment variables are set by Pixi, when using the `pixi run`, `pixi shell`, or `pixi shell-hook` command:

- `PIXI_PROJECT_ROOT`: The root directory of the project.
- `PIXI_PROJECT_NAME`: The name of the project.
- `PIXI_PROJECT_MANIFEST`: The path to the manifest file (`pixi.toml`).
- `PIXI_PROJECT_VERSION`: The version of the project.
- `PIXI_PROMPT`: The prompt to use in the shell, also used by `pixi shell` itself.
- `PIXI_ENVIRONMENT_NAME`: The name of the environment, defaults to `default`.
- `PIXI_ENVIRONMENT_PLATFORMS`: Comma separated list of platforms supported by the project.
- `CONDA_PREFIX`: The path to the environment. (Used by multiple tools that already understand conda environments)
- `CONDA_DEFAULT_ENV`: The name of the environment. (Used by multiple tools that already understand conda environments)
- `PATH`: We prepend the `bin` directory of the environment to the `PATH` variable, so you can use the tools installed in the environment directly.
- `INIT_CWD`: ONLY IN `pixi run`: The directory where the command was run from.

!!! note
    Even though the variables are environment variables these cannot be overridden. E.g. you can not change the root of the project by setting `PIXI_PROJECT_ROOT` in the environment.
