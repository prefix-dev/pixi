# The CLI commands of pixi
With `pixi` you can install packages in global space or local to the environment in a project.

## Pixi Project Commands

| Command   | Use case                                                    |
|-----------|-------------------------------------------------------------|
| `init`    | Creates a new project by initializing a `pixi.toml` file.   |
| `add`     | Adds a dependency to the project file.                      |
| `install` | Installs all dependencies of the project in its environment |
| `run`     | Runs the given command in a project's environment           |
| `shell`   | Starts a shell in the project's environment                 |
| `tasks`   | Manage tasks in your `pixi.toml` file                       |
| `auth`    | Authenticate pixi with a server to access private channels  |

### Initialize a new project
This command is used to create a new project.
It initializes a pixi.toml file and also prepares a `.gitignore` to prevent the environment from being added to `git`.
```bash
pixi init myproject
pixi init ~/myproject
pixi init  # Initializes directly in the current directory.
pixi init --channel conda-forge --channel bioconda myproject
```

### Add dependencies to the project
Adds dependencies to the `pixi.toml` before it does that it check if it is possible to solve the environment.
It will only add if the package with its version constraint is able to work with rest of the dependencies in the project.
```bash
pixi add numpy
pixi add numpy pandas pytorch>=1.8
pixi add "numpy>=1.22,<1.24"
pixi add --manifest-path ~/myproject/pixi.toml numpy
pixi add --host python>=3.9.0
pixi add --build cmake
```

### Install the dependencies
Installs all dependencies specified in the lockfile `pixi.lock`.
Which gets generated on `pixi add` or when you manually change the `pixi.toml` file and run `pixi install`.
```bash
pixi install
pixi install --manifest-path ~/myproject/pixi.toml
```

### Run commands in the environment
The `run` commands first checks if the environment is ready to use.
When you didn't run `pixi install` the run command will do that for you.
The custom tasks defined in the `pixi.toml` are also available through the run command.

**NOTE:** In `pixi` the [`deno_task_shell`](https://deno.land/manual@v1.35.0/tools/task_runner#task-runner) is the underlying runner of the run command.
Checkout their [documentation](https://deno.land/manual@v1.35.0/tools/task_runner#task-runner) for the syntax and available commands.
This is done so that the run commands can be run across all platforms.

You cannot run `pixi run source setup.bash` as `source` is not available in the `deno_task_shell` commandos and not an executable.

```bash
pixi run python
pixi run cowpy "Hey pixi user"
pixi run --manifest-path ~/myproject/pixi.toml python
# If you have specified a custom task in the pixi.toml you can run it with run as well
pixi run build
```

### Create a task from a command
**NOTE:** In `pixi` the [`deno_task_shell`](https://deno.land/manual@v1.35.0/tools/task_runner#task-runner) is the underlying runner of the tasks.
Checkout their [documentation](https://deno.land/manual@v1.35.0/tools/task_runner#task-runner) for the syntax and available commands.
This is done so that the tasks defined can be run across all platforms.

If you want to make a shorthand for a specific command you can add a task for it
```bash
pixi task add cow cowpy "Hello User"
```

This adds the following to the `pixi.toml`:

```toml
[tasks]
cow = "cowpy \"Hello User\""
```
Which you can then run with the `run` command:

```bash
pixi run cow
```

To remove a task you can use the `task remove` command:

```bash
pixi task remove cow
```

### Start a shell in the environment
This command starts a new shell in the project's environment.
To exit the pixi shell, simply run exit
```bash
pixi shell
exit
pixi shell --manifest-path ~/myproject/pixi.toml
exit
```


## Pixi Global Commands

| Command          | Use case                                                                                                                            |
|------------------|-------------------------------------------------------------------------------------------------------------------------------------|
| `auth`           | Authenticate on user level the access to remote hosts like `prefix.dev` or `anaconda.org` for the use of private channels.          |
| `completion`     | Generates the shell completion scripts to enable tab completion.                                                                    |
| `global install` | Installs a package into its own environment and adds the binary to `PATH` so it can be accessed without activating the environment. |

### Authenticate pixi to access package repository hosts
This command is used to authenticate the user's access to remote hosts such as `prefix.dev` or `anaconda.org` for private channels.
```bash
pixi auth login repo.prefix.dev --token pfx_JQEV-m_2bdz-D8NSyRSaNdHANx0qHjq7f2iD
pixi auth login anaconda.org --conda-token ABCDEFGHIJKLMNOP
pixi auth login https://myquetz.server --user john --password xxxxxx

pixi auth logout repo.prefix.dev
pixi auth logout anaconda.org
```

### Add the completion scripts for your shell
This command generates the shell completion scripts to enable tab completion. The completion command outputs the scripts to the command line, and with an eval in the config file of your shell, it retrieves the latest version each time you source your shell.
```bash
# On unix (macOS or Linux), pick your shell (use `echo $SHELL` to find the shell you are using.):
echo 'eval "$(pixi completion --shell bash)"' >> ~/.bashrc
echo 'eval "$(pixi completion --shell zsh)"' >> ~/.zshrc
echo 'pixi completion --shell fish | source' >> ~/.config/fish/config.fish
echo 'eval (pixi completion --shell elvish | slurp)' >> ~/.elvish/rc.elv

# On Windows:
Add-Content -Path $PROFILE -Value 'Invoke-Expression (&pixi completion --shell powershell)'
```

### Install a tool or package globally
This command installs a package into its own environment and adds the binary to `PATH`, allowing you to access it anywhere on your system without activating the environment.
```bash
pixi global install ruff
pixi global install starship
pixi global install --channel conda-forge --channel bioconda trackplot
# Or in a more concise form
pixi global install -c conda-forge -c bioconda trackplot

# Support full conda matchspec
pixi global install python=3.9.*
pixi global install "python [version="3.11.0", build_number=1]"
pixi global install "python [version="3.11.0", build=he550d4f_1_cpython]"
pixi global install python=3.11.0=h10a6764_1_cpython
```
After using global install you can use the package you installed anywhere on your system.
