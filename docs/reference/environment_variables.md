
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


## Priority of Environment Variables

The following priority rule applies for environment variables: `task.env` > `activation.env` > `activation.scripts` > activation scripts of dependencies > outside environment variable.
Variables defined at a higher priority will override those defined at a lower priority.

##### Example 1:  `task.env` > `activation.env`

In `pixi.toml`, we defined an environment variable `HELLO_WORLD` in both `tasks.hello` and `activation.env`. 

When we run `echo $HELLO_WORLD`, it will output:
```
Hello world!
```

```toml
# pixi.toml
[tasks.hello]
cmd = "echo $HELLO_WORLD"
env = { HELLO_WORLD = "Hello world!" }
[activation.env]
HELLO_WORLD = "Activate!"
```

##### Example 2: `activation.env` > `activation.scripts`

In `pixi.toml`, we defined the same environment variable `DEBUG_MODE` in both `activation.env` and in the activation script file `setup.sh`.
When we run `echo Debug mode: $DEBUG_MODE`, it will output:
```bash
Debug mode: enabled
```

```toml
# pixi.toml
[activation.env]
DEBUG_MODE = "enabled"

[activation]
scripts = ["setup.sh"]
```

```bash
# setup.sh
export DEBUG_MODE="disabled"
```

##### Example 3: `activation.scripts` > activation scripts of dependencies

In `pixi.toml`, we have our local activation script and a dependency `my-package` that also sets environment variables through its activation scripts.
When we run `echo Library path: $LIB_PATH`, it will output:
```
Library path: /my/lib
```

```toml
# pixi.toml
[activation]
scripts = ["local_setup.sh"]

[dependencies]
my-package = "*"  # This package has its own activation scripts that set LIB_PATH="/dep/lib"
```
```bash
# local_setup.sh
export LIB_PATH="/my/lib"
```

##### Example 4: activation scripts of dependencies > outside environment variable

If we have a dependency that sets `PYTHON_PATH` and the same variable is already set in the outside environment.
When we run `echo Python path: $PYTHON_PATH`, it will output:
```bash
Python path: /pixi/python
```
```
# Outside environment
export PYTHON_PATH="/system/python"
```
```toml
# pixi.toml
[dependencies]
python-utils = "*"  # This package sets PYTHON_PATH="/pixi/python" in its activation scripts
```

##### Example 5: Complex Example - All priorities combined
In `pixi.toml`, we define the same variable `APP_CONFIG` across multiple levels:
```toml
[tasks.start]
cmd = "echo Config: $APP_CONFIG"
env = { APP_CONFIG = "task-specific" }

[activation.env]
APP_CONFIG = "activation-env"

[activation]
scripts = ["app_setup.sh"]

[dependencies]
config-loader = "*"  # Sets APP_CONFIG="dependency-config"
```
```bash
# app_setup.sh
export APP_CONFIG="activation-script"
```
```bash
# Outside environment
export APP_CONFIG="system-config"
```

Since `task.env` has the highest priority, when we run `pixi run start` it will output:

```
Config: task-specific
```
