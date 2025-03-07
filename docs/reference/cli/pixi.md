# <code>pixi</code>

--8<-- "docs/reference/cli/pixi_extender.md:description"

## Usage
```
pixi [OPTIONS] <COMMAND>
```

## Subcommands
| Command | Description |
|---------|-------------|
| [`init`](init) | Creates a new workspace |
| [`add`](add) | Adds dependencies to the project |
| [`remove`](remove) | Removes dependencies from the project |
| [`install`](install) | Install all dependencies |
| [`update`](update) | Update dependencies as recorded in the local lock file |
| [`upgrade`](upgrade) | Update the version of packages to the latest possible version, disregarding the manifest version constraints |
| [`lock`](lock) | Solve environment and update the lock file |
| [`run`](run) | Runs task in project |
| [`exec`](exec) | Run a command in a temporary environment |
| [`shell`](shell) | Start a shell in the pixi environment of the project |
| [`shell-hook`](shell-hook) | Print the pixi environment activation script |
| [`project`](project) | Modify the project configuration file through the command line |
| [`task`](task) | Interact with tasks in the project |
| [`list`](list) | List project's packages |
| [`tree`](tree) | Show a tree of project dependencies |
| [`global`](global) | Subcommand for global package management actions |
| [`auth`](auth) | Login to prefix.dev or anaconda.org servers to access private channels |
| [`config`](config) | Configuration management |
| [`info`](info) | Information about the system, project and environments for the current machine |
| [`upload`](upload) | Upload a conda package |
| [`search`](search) | Search a conda package |
| [`clean`](clean) | Clean the parts of your system which are touched by pixi. Defaults to cleaning the environments and task cache. Use the `cache` subcommand to clean the cache |
| [`completion`](completion) | Generates a completion script for a shell |
| [`build`](build) | Workspace configuration |


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
