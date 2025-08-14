When building a package, you often have to do more than just run the code.
Steps like formatting, linting, compiling, testing, benchmarking, etc. are often part of a workspace.
With Pixi tasks, this should become much easier to do.

Here are some quick examples

```toml title="pixi.toml"
[tasks]
# Commands as lists so you can also add documentation in between.
configure = { cmd = [
    "cmake",
    # Use the cross-platform Ninja generator
    "-G",
    "Ninja",
    # The source is in the root directory
    "-S",
    ".",
    # We wanna build in the .build directory
    "-B",
    ".build",
] }

# Depend on other tasks
build = { cmd = ["ninja", "-C", ".build"], depends-on = ["configure"] }

# Using environment variables
run = "python main.py $PIXI_PROJECT_ROOT"
set = "export VAR=hello && echo $VAR"

# Cross platform file operations
copy = "cp pixi.toml pixi_backup.toml"
clean = "rm pixi_backup.toml"
move = "mv pixi.toml backup.toml"
```

## Depends on

Just like packages can depend on other packages, our tasks can depend on other tasks.
This allows for complete pipelines to be run with a single command.

An obvious example is **compiling** before **running** an application.

Checkout our [`cpp_sdl` example](https://github.com/prefix-dev/pixi/tree/main/examples/cpp-sdl) for a running example.
In that package we have some tasks that depend on each other, so we can assure that when you run `pixi run start` everything is set up as expected.

```fish
pixi task add configure "cmake -G Ninja -S . -B .build"
pixi task add build "ninja -C .build" --depends-on configure
pixi task add start ".build/bin/sdl_example" --depends-on build
```

Results in the following lines added to the `pixi.toml`

```toml title="pixi.toml"
[tasks]
# Configures CMake
configure = "cmake -G Ninja -S . -B .build"
# Build the executable but make sure CMake is configured first.
build = { cmd = "ninja -C .build", depends-on = ["configure"] }
# Start the built executable
start = { cmd = ".build/bin/sdl_example", depends-on = ["build"] }
```

The tasks will be executed after each other:

- First `configure` because it has no dependencies.
- Then `build` as it only depends on `configure`.
- Then `start` as all its dependencies are run.

If one of the commands fails (exit with non-zero code.) it will stop and the next one will not be started.

With this logic, you can also create aliases as you don't have to specify any command in a task.

```shell
pixi task add fmt ruff
pixi task add lint pylint
```

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_tomls/pixi_task_alias.toml:not-all"
```

!!! tip "Hiding Tasks"
    Tasks can be hidden from user facing commands by [naming them](#task-names) with an `_` prefix.

### Shorthand Syntax

Pixi supports a shorthand syntax for defining tasks that only depend on other tasks. Instead of using the more verbose `depends-on` field, you can define a task directly as an array of dependencies.

Executing:

```
pixi task alias style fmt lint
```

results in the following `pixi.toml`:

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_tomls/pixi_task_alias.toml:all"
```

Now you can run both tools with one command.

```shell
pixi run style
```

### Environment specification for task dependencies

You can specify the environment to use for a dependent task:

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_tomls/tasks_depends_on.toml:tasks"
```

This allows you to run tasks in different environments as part of a single pipeline.
When you run the main task, Pixi ensures each dependent task uses its specified environment:

```shell
pixi run test-all
```

The environment specified for a task dependency takes precedence over the environment specified via the CLI `--environment` flag. This means even if you run `pixi run test-all --environment py312`, the first dependency will still run in the `py311` environment as specified in the TOML file.

In the example above, the `test-all` task runs the `test` task in both Python 3.11 and 3.12 environments, allowing you to verify compatibility across different Python versions with a single command.

## Working directory

Pixi tasks support the definition of a working directory.

`cwd` stands for Current Working Directory.
The directory is relative to the Pixi workspace root, where the `pixi.toml` file is located.

By default, tasks are executed from the Pixi workspace root.
To change this, use the `--cwd` flag.
For example, consider a Pixi workspace structured as follows:

```shell
├── pixi.toml
└── scripts
    └── bar.py
```

To add a task that runs the `bar.py` file from the `scripts` directory, use:

```shell
pixi task add bar "python bar.py" --cwd scripts
```

This will add the following line to [manifest file](../reference/pixi_manifest.md):

```toml title="pixi.toml"
[tasks]
bar = { cmd = "python bar.py", cwd = "scripts" }
```

## Task Arguments

Tasks can accept arguments that can be referenced in the command. This provides more flexibility and reusability for your tasks.

### Why Use Task Arguments?

Task arguments make your tasks more versatile and maintainable:

- **Reusability**: Create generic tasks that can work with different inputs rather than duplicating tasks for each specific case
- **Flexibility**: Change behavior at runtime without modifying your pixi.toml file
- **Clarity**: Make your task intentions clear by explicitly defining what values can be customized
- **Validation**: Define required arguments to ensure tasks are called correctly
- **Default values**: Set sensible defaults while allowing overrides when needed

For example, instead of creating separate build tasks for development and production modes, you can create a single parameterized task that handles both cases.

Arguments can be:

- **Required**: must be provided when running the task
- **Optional**: can have default values that are used when not explicitly provided

### Defining Task Arguments

Define arguments in your task using the `args` field:

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_tomls/task_arguments.toml:project_tasks"
```

!!! note "Argument naming restrictions"
    Argument names cannot contain dashes (`-`) due to them being seen as a minus sign in MiniJinja. Use underscores (`_`) or camelCase instead.

### Using Task Arguments

When running a task, provide arguments in the order they are defined:

```shell
# Required argument
pixi run greet John
✨ Pixi task (greet in default): echo Hello, John!

# Default values are used when omitted
pixi run build
✨ Pixi task (build in default): echo Building my-app in development mode

# Override default values
pixi run build my-project production
✨ Pixi task (build in default): echo Building my-project in production mode

# Mixed argument types
pixi run deploy auth-service
✨ Pixi task (deploy in default): echo Deploying auth-service to staging
pixi run deploy auth-service production
✨ Pixi task (deploy in default): echo Deploying auth-service to production
```
### Passing Arguments to Dependent Tasks

You can pass arguments to tasks that are dependencies of other tasks:

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_tomls/task_arguments_dependent.toml:project_tasks"
```

When executing a dependent task, the arguments are passed to the dependency:

```shell
pixi run install-release
✨ Pixi task (install in default): echo Installing with manifest /path/to/manifest and flag --debug

pixi run deploy
✨ Pixi task (install in default): echo Installing with manifest /custom/path and flag --verbose
✨ Pixi task (deploy in default): echo Deploying
```

When a dependent task doesn't specify all arguments, the default values are used for the missing ones:

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_tomls/task_arguments_partial.toml:project_tasks"
```

```shell
pixi run partial-override
✨ Pixi task (base-task in default): echo Base task with override1 and default2
```

For a dependent task to accept arguments to pass to the dependency, you can use the same syntax as passing arguments to the command:

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_tomls/task_arguments_partial.toml:project_tasks_with_arg"
```

```shell
pixi run partial-override-with-arg
✨ Pixi task (base-task in default): echo Base task with override1 and new-default2
pixi run partial-override-with-arg cli-arg
✨ Pixi task (base-task in default): echo Base task with override1 and cli-arg
```

### MiniJinja Templating for Task Arguments

Task commands support MiniJinja templating syntax for accessing and formatting argument values. This provides powerful flexibility when constructing commands.

Basic syntax for using an argument in your command:

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_tomls/task_minijinja_simple.toml:tasks"
```

You can also use filters to transform argument values:

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/minijinja/task_args/pixi.toml:tasks"
```

For more information about available filters and template syntax, see the [MiniJinja documentation](https://docs.rs/minijinja/latest/minijinja/filters/index.html).

## Task Names

A task name follows these rules:

- **No spaces** are allowed in the name.
- Must **be unique** within the table.
- [`_`]("underscores") at the start of the name will **hide** the task from the `pixi task list` command.

Hiding tasks can be useful if your project defines many tasks but your users only need to use a subset of them.

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_tomls/task_visibility.toml:project_tasks"
```

## Caching

When you specify `inputs` and/or `outputs` to a task, Pixi will reuse the result of the task.

For the cache, Pixi checks that the following are true:

- No package in the environment has changed.
- The selected inputs and outputs are the same as the last time the task was
  run. We compute fingerprints of all the files selected by the globs and
  compare them to the last time the task was run.
- The command is the same as the last time the task was run.

If all of these conditions are met, Pixi will not run the task again and instead use the existing result.

Inputs and outputs can be specified as globs, which will be expanded to all matching files. You can also use MiniJinja templates in your `inputs` and `outputs` fields to parameterize the paths, making tasks more reusable:

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_tomls/tasks_minijinja_inputs_outputs.toml:tasks"
```

When using template variables in inputs/outputs, Pixi expands the templates using the provided arguments or environment variables, and uses the resolved paths for caching decisions. This allows you to create generic tasks that can handle different files without duplicating task configurations:

```shell
# First run processes the file and caches the result
pixi run process-file data1

# Second run with the same argument uses the cached result
pixi run process-file data1  # [cache hit]

# Run with a different argument processes a different file
pixi run process-file data2
```

Note: if you want to debug the globs you can use the `--verbose` flag to see which files are selected.

```shell
# shows info logs of all files that were selected by the globs
pixi run -v start
```

## Environment variables
You can set environment variables for a task.
These are seen as "default" values for the variables as you can overwrite them from the shell.

```toml title="pixi.toml"
[tasks]
echo = { cmd = "echo $ARGUMENT", env = { ARGUMENT = "hello" } }
```
If you run `pixi run echo` it will output `hello`.
When you set the environment variable `ARGUMENT` before running the task, it will use that value instead.

```shell
ARGUMENT=world pixi run echo
✨ Pixi task (echo in default): echo $ARGUMENT
world
```

These variables are not shared over tasks, so you need to define these for every task you want to use them in.

!!! note "Extend instead of overwrite"
    If you use the same environment variable in the value as in the key of the map you will also overwrite the variable.
    For example overwriting a `PATH`
    ```toml title="pixi.toml"
    [tasks]
    echo = { cmd = "echo $PATH", env = { PATH = "/tmp/path:$PATH" } }
    ```
    This will output `/tmp/path:/usr/bin:/bin` instead of the original `/usr/bin:/bin`.

## Clean environment
You can make sure the environment of a task is "Pixi only".
Here Pixi will only include the minimal required environment variables for your platform to run the command in.
The environment will contain all variables set by the conda environment like `"CONDA_PREFIX"`.
It will however include some default values from the shell, like:
`"DISPLAY"`, `"LC_ALL"`, `"LC_TIME"`, `"LC_NUMERIC"`, `"LC_MEASUREMENT"`, `"SHELL"`, `"USER"`, `"USERNAME"`, `"LOGNAME"`, `"HOME"`, `"HOSTNAME"`,`"TMPDIR"`, `"XPC_SERVICE_NAME"`, `"XPC_FLAGS"`

```toml
[tasks]
clean_command = { cmd = "python run_in_isolated_env.py", clean-env = true}
```
This setting can also be set from the command line with `pixi run --clean-env TASK_NAME`.

!!! warning "`clean-env` not supported on Windows"
    On Windows it's hard to create a "clean environment" as `conda-forge` doesn't ship Windows compilers and Windows needs a lot of base variables.
    Making this feature not worthy of implementing as the amount of edge cases will make it unusable.



## Our task runner: deno_task_shell

To support the different OS's (Windows, OSX and Linux), Pixi integrates a shell that can run on all of them.
This is [`deno_task_shell`](https://deno.land/manual@v1.35.0/tools/task_runner#built-in-commands).
The task shell is a limited implementation of a bourne-shell interface.

### Built-in commands

Next to running actual executable like `./myprogram`, `cmake` or `python` the shell has some built-in commands.

- `cp`: Copies files.
- `mv`: Moves files.
- `rm`: Remove files or directories.
  Ex: `rm -rf [FILE]...` - Commonly used to recursively delete files or directories.
- `mkdir`: Makes directories.
  Ex. `mkdir -p DIRECTORY...` - Commonly used to make a directory and all its parents with no error if it exists.
- `pwd`: Prints the name of the current/working directory.
- `sleep`: Delays for a specified amount of time.
  Ex. `sleep 1` to sleep for 1 second, `sleep 0.5` to sleep for half a second, or `sleep 1m` to sleep a minute
- `echo`: Displays a line of text.
- `cat`: Concatenates files and outputs them on stdout. When no arguments are provided, it reads and outputs stdin.
- `exit`: Causes the shell to exit.
- `unset`: Unsets environment variables.
- `xargs`: Builds arguments from stdin and executes a command.

### Syntax

- **Boolean list:** use `&&` or `||` to separate two commands.
  - `&&`: if the command before `&&` succeeds continue with the next command.
  - `||`: if the command before `||` fails continue with the next command.
- **Sequential lists:** use `;` to run two commands without checking if the first command failed or succeeded.
- **Environment variables:**
  - Set env variable using: `export ENV_VAR=value`
  - Use env variable using: `$ENV_VAR`
  - unset env variable using `unset ENV_VAR`
- **Shell variables:** Shell variables are similar to environment variables, but won't be exported to spawned commands.
  - Set them: `VAR=value`
  - use them: `VAR=value && echo $VAR`
- **Pipelines:** Use the stdout output of a command into the stdin a following command
  - `|`: `echo Hello | python receiving_app.py`
  - `|&`: use this to also get the stderr as input.
- **Command substitution:** `$()` to use the output of a command as input for another command.
  - `python main.py $(git rev-parse HEAD)`
- **Negate exit code:** `! ` before any command will negate the exit code from 1 to 0 or visa-versa.
- **Redirects:** `>` to redirect the stdout to a file.
  - `echo hello > file.txt` will put `hello` in `file.txt` and overwrite existing text.
  - `python main.py 2> file.txt` will put the `stderr` output in `file.txt`.
  - `python main.py &> file.txt` will put the `stderr` **and** `stdout` in `file.txt`.
  - `echo hello >> file.txt` will append `hello` to the existing `file.txt`.
- **Glob expansion:** `*` to expand all options.
  - `echo *.py` will echo all filenames that end with `.py`
  - `echo **/*.py` will echo all filenames that end with `.py` in this directory and all descendant directories.
  - `echo data[0-9].csv` will echo all filenames that have a single number after `data` and before `.csv`

More info in [`deno_task_shell` documentation](https://deno.land/manual@v1.35.0/tools/task_runner#task-runner).
