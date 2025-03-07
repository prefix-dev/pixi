# <code>pixi</code>

--8<-- "docs/reference/cli/pixi_extender.md:description"

## Usage
```
pixi [OPTIONS] <COMMAND>
```

## Subcommands
| Command | Description |
|---------|-------------|
| [`init`](pixi/init.md) | Creates a new workspace |
| [`add`](pixi/add.md) | Adds dependencies to the project |
| [`remove`](pixi/remove.md) | Removes dependencies from the project |
| [`install`](pixi/install.md) | Install all dependencies |
| [`update`](pixi/update.md) | Update dependencies as recorded in the local lock file |
| [`upgrade`](pixi/upgrade.md) | Update the version of packages to the latest possible version, disregarding the manifest version constraints |
| [`lock`](pixi/lock.md) | Solve environment and update the lock file |
| [`run`](pixi/run.md) | Runs task in project |
| [`exec`](pixi/exec.md) | Run a command in a temporary environment |
| [`shell`](pixi/shell.md) | Start a shell in the pixi environment of the project |
| [`shell-hook`](pixi/shell-hook.md) | Print the pixi environment activation script |
| [`project`](pixi/project.md) | Modify the project configuration file through the command line |
| [`task`](pixi/task.md) | Interact with tasks in the project |
| [`list`](pixi/list.md) | List project's packages |
| [`tree`](pixi/tree.md) | Show a tree of project dependencies |
| [`global`](pixi/global.md) | Subcommand for global package management actions |
| [`auth`](pixi/auth.md) | Login to prefix.dev or anaconda.org servers to access private channels |
| [`config`](pixi/config.md) | Configuration management |
| [`info`](pixi/info.md) | Information about the system, project and environments for the current machine |
| [`upload`](pixi/upload.md) | Upload a conda package |
| [`search`](pixi/search.md) | Search a conda package |
| [`clean`](pixi/clean.md) | Clean the parts of your system which are touched by pixi. Defaults to cleaning the environments and task cache. Use the `cache` subcommand to clean the cache |
| [`completion`](pixi/completion.md) | Generates a completion script for a shell |
| [`build`](pixi/build.md) | Workspace configuration |


## Options
- <a id="option-version" href="#option-version">`--version (-V)`</a>
: Display version information

## Global Options
- <a id="arg---color" href="#arg---color">`--color <COLOR>`</a>
:  Whether the log needs to be colored
<br>**env**: `PIXI_COLOR`
<br>**default**: `auto`
<br>**options**: `always`, `never`, `auto`
- <a id="arg---no-progress" href="#arg---no-progress">`--no-progress`</a>
:  Hide all progress bars, always turned on if stderr is not a terminal
<br>**env**: `PIXI_NO_PROGRESS`
<br>**default**: `false`
- <a id="arg---quiet" href="#arg---quiet">`--quiet (-q)`</a>
:  Decrease logging verbosity
- <a id="arg---verbose" href="#arg---verbose">`--verbose (-v)`</a>
:  Increase logging verbosity

--8<-- "docs/reference/cli/pixi_extender.md:example"
