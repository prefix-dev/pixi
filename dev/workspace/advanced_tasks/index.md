When building a package, you often have to do more than just run the code. Steps like formatting, linting, compiling, testing, benchmarking, etc. are often part of a workspace. With Pixi tasks, this should become much easier to do.

Here are some quick examples

pixi.toml

```toml
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

Just like packages can depend on other packages, our tasks can depend on other tasks. This allows for complete pipelines to be run with a single command.

An obvious example is **compiling** before **running** an application.

Checkout our [`cpp_sdl` example](https://github.com/prefix-dev/pixi/tree/main/examples/cpp-sdl) for a running example. In that package we have some tasks that depend on each other, so we can assure that when you run `pixi run start` everything is set up as expected.

```fish
pixi task add configure "cmake -G Ninja -S . -B .build"
pixi task add build "ninja -C .build" --depends-on configure
pixi task add start ".build/bin/sdl_example" --depends-on build

```

Results in the following lines added to the `pixi.toml`

pixi.toml

```toml
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

pixi.toml

```toml
[tasks]
fmt = "ruff"
lint = "pylint"

```

Hiding Tasks

Tasks can be hidden from user facing commands by [naming them](#task-names) with an `_` prefix.

### Shorthand Syntax

Pixi supports a shorthand syntax for defining tasks that only depend on other tasks. Instead of using the more verbose `depends-on` field, you can define a task directly as an array of dependencies.

Executing:

```text
pixi task alias style fmt lint

```

results in the following `pixi.toml`:

pixi.toml

```toml
[tasks]
fmt = "ruff"
lint = "pylint"
style = [{ task = "fmt" }, { task = "lint" }]

```

Now you can run both tools with one command.

```shell
pixi run style

```

### Environment specification for task dependencies

You can specify the environment to use for a dependent task:

pixi.toml

```toml
[tasks]
test = "python --version"
[feature.py311.dependencies]
python = "3.11.*"
[feature.py312.dependencies]
python = "3.12.*"
[environments]
py311 = ["py311"]
py312 = ["py312"]
# Task that depends on other tasks in different environments
[tasks.test-all]
depends-on = [
  { task = "test", environment = "py311" },
  { task = "test", environment = "py312" },
]

```

This allows you to run tasks in different environments as part of a single pipeline. When you run the main task, Pixi ensures each dependent task uses its specified environment:

```shell
pixi run test-all

```

The environment specified for a task dependency takes precedence over the environment specified via the CLI `--environment` flag. This means even if you run `pixi run test-all --environment py312`, the first dependency will still run in the `py311` environment as specified in the TOML file.

In the example above, the `test-all` task runs the `test` task in both Python 3.11 and 3.12 environments, allowing you to verify compatibility across different Python versions with a single command.

## Working directory

Pixi tasks support the definition of a working directory.

`cwd` stands for Current Working Directory. The directory is relative to the Pixi workspace root, where the `pixi.toml` file is located.

By default, tasks are executed from the Pixi workspace root. To change this, use the `--cwd` flag. For example, consider a Pixi workspace structured as follows:

```shell
├── pixi.toml
└── scripts
    └── bar.py

```

To add a task that runs the `bar.py` file from the `scripts` directory, use:

```shell
pixi task add bar "python bar.py" --cwd scripts

```

This will add the following line to [manifest file](../../reference/pixi_manifest/):

pixi.toml

```toml
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

pixi.toml

```toml
# Task with required arguments
[tasks.greet]
args = ["name"]
cmd = "echo Hello, {{ name }}!"
# Task with optional arguments (default values)
[tasks.build]
args = [
  { "arg" = "project", "default" = "my-app" },
  { "arg" = "mode", "default" = "development" },
]
cmd = "echo Building {{ project }} with {{ mode }} mode"
# Task with mixed required and optional arguments
[tasks.deploy]
args = ["service", { "arg" = "environment", "default" = "staging" }]
cmd = "echo Deploying {{ service }} to {{ environment }}"

```

Argument naming restrictions

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

pixi.toml

```toml
# Base task with arguments
[tasks.install]
args = [
  { arg = "path", default = "/default/path" }, # Path to manifest
  { arg = "flag", default = "--normal" },      # Installation flag
]
cmd = "echo Installing with manifest {{ path }} and flag {{ flag }}"
# Dependent task specifying positional arguments for the base task
[tasks.install-release]
depends-on = [{ task = "install", args = ["/path/to/manifest", "--debug"] }]
# Task with multiple dependencies, passing different arguments
[tasks.deploy]
cmd = "echo Deploying"
depends-on = [
  # Override with named custom path and named verbose flag
  { task = "install", args = [
    { path = "/custom/path" },
    { flag = "--verbose" },
  ] },
  # Other dependent tasks can be added here
]

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

pixi.toml

```toml
[tasks.base-task]
args = [
  { "arg" = "arg1", "default" = "default1" }, # First argument with default
  { "arg" = "arg2", "default" = "default2" }, # Second argument with default
]
cmd = "echo Base task with {{ arg1 }} and {{ arg2 }}"
[tasks.partial-override]
# Only override the first argument
depends-on = [{ "task" = "base-task", "args" = ["override1"] }]

```

```shell
pixi run partial-override
✨ Pixi task (base-task in default): echo Base task with override1 and default2

```

For a dependent task to accept arguments to pass to the dependency, you can use the same syntax as passing arguments to the command:

pixi.toml

```toml
[tasks.base-task]
args = [
  { "arg" = "arg1", "default" = "default1" }, # First argument with default
  { "arg" = "arg2", "default" = "default2" }, # Second argument with default
]
cmd = "echo Base task with {{ arg1 }} and {{ arg2 }}"
[tasks.partial-override]
# Only override the first argument
depends-on = [{ "task" = "base-task", "args" = ["override1"] }]
[tasks.partial-override-with-arg]
# Only override the first argument
args = [
  { arg = "arg2", default = "new-default2" }, # Argument with new default
]
depends-on = [{ task = "base-task", args = ["override1", "{{ arg2 }}"] }]

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

pixi.toml

```toml
[tasks]
greet = { cmd = "echo Hello, {{ name }}!", args = ["name"] }

```

You can also use filters to transform argument values:

pixi.toml

```toml
[tasks]
# The arg `text`, converted to uppercase, will be printed.
task1 = { cmd = "echo {{ text | upper }}", args = ["text"] }
# If arg `text` contains 'hoi', it will be converted to lowercase. The result will be printed.
task2 = { cmd = "echo {{ text | lower if 'hoi' in text }}", args = [
  { arg = "text", default = "" },
] }
# With `a` and `b` being strings, they will be appended and then printed.
task3 = { cmd = "echo {{ a + b }}", args = ["a", { arg = "b", default = "!" }] }
# If the string "win" is in arg `platform`, "windows" will be printed, otherwise "unix".
task4 = { cmd = """echo {% if "win" in platform  %}windows{% else %}unix{% endif %}""", args = [
  "platform",
] }
# `names` will be split by whitespace and then every name will be printed separately
task5 = { cmd = "{% for name in names | split %} echo {{ name }};{% endfor %}", args = [
  "names",
] }

```

For more information about available filters and template syntax, see the [MiniJinja documentation](https://docs.rs/minijinja/latest/minijinja/filters/index.html).

## Task Names

A task name follows these rules:

- **No spaces** are allowed in the name.
- Must **be unique** within the table.
- `_` at the start of the name will **hide** the task from the `pixi task list` command.

Hiding tasks can be useful if your workspace defines many tasks but your users only need to use a subset of them.

pixi.toml

```toml
# Hidden task that is only intended to be used by other tasks
[tasks._git-clone]
args = ["url"]
cmd = "echo git clone {{ url }}"
# Hidden task that clones a dependency
[tasks._clone-subproject]
depends-on = [
  { task = "_git-clone", args = [
    "https://git.hub/org/subproject.git",
  ] },
]
# Task to build the project which depends on cloning a dependency
[tasks.build]
cmd = "echo Building project"
depends-on = ["_clone-subproject"]

```

## Caching

When you specify `inputs` and/or `outputs` to a task, Pixi will reuse the result of the task.

For the cache, Pixi checks that the following are true:

- No package in the environment has changed.
- The selected inputs and outputs are the same as the last time the task was run. We compute fingerprints of all the files selected by the globs and compare them to the last time the task was run.
- The command is the same as the last time the task was run.

If all of these conditions are met, Pixi will not run the task again and instead use the existing result.

Inputs and outputs can be specified as globs, which will be expanded to all matching files. You can also use MiniJinja templates in your `inputs` and `outputs` fields to parameterize the paths, making tasks more reusable:

pixi.toml

```toml
[tasks]
# This task will only run if the `main.py` file has changed.
run = { cmd = "python main.py", inputs = ["main.py"] }
# This task will remember the result of the `curl` command and not run it again if the file `data.csv` already exists.
download_data = { cmd = "curl -o data.csv https://example.com/data.csv", outputs = [
  "data.csv",
] }
# This task will only run if the `src` directory has changed and will remember the result of the `make` command.
build = { cmd = "make", inputs = [
  "src/*.cpp",
  "include/*.hpp",
], outputs = [
  "build/app.exe",
] }
# Process a specific file based on the provided argument
process-file = { cmd = "python process.py inputs/{{ filename }}.txt --output outputs/{{ filename }}.processed", args = [
  "filename",
], inputs = [
  "inputs/{{ filename }}.txt",
], outputs = [
  "outputs/{{ filename }}.processed",
] }

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

## Environment Variables

You can set environment variables directly for a task, as well as by other means. See [the environment variable priority documentation](../../reference/environment_variables/#environment-variable-priority) for full details of ways to set environment variables, and how those ways interact with each other.

Notes on environment variables in tasks:

- Values set via `tasks.<name>.env` are interpreted by `deno_task_shell` when the task runs. Shell-style expansions like `env = { VAR = "$FOO" }` therefore work the same on all operating systems.

Warning

In older versions of Pixi, this priority was not well-defined, and there are a number of known deviations from the current priority which exist in some older versions:

- `activation.scripts` used to take priority over `activation.env`
- activation scripts of dependencies used to take priority over `activation.env`
- outside environment variables used to override variables set in `task.env`

If you previously relied on a certain priority which no longer applies, you may need to change your task definitions.

For the specific case of overriding `task.env` with outside environment variables, this behaviour can now be recreated using [task arguments](#task-arguments). For example, if you were previously using a setup like:

pixi.toml

```toml
[tasks]
echo = { cmd = "echo $ARGUMENT", env = { ARGUMENT = "hello" } }

```

```shell
ARGUMENT=world pixi run echo
✨ Pixi task (echo in default): echo $ARGUMENT
world

```

you can now recreate this behaviour like:

pixi.toml

```toml
[tasks]
echo = { cmd = "echo {{ ARGUMENT }}", args = [{"arg" = "ARGUMENT", "default" = "hello" }] }

```

```shell
pixi run echo world
✨ Pixi task (echo): echo world
world

```

## Clean environment

You can make sure the environment of a task is "Pixi only". Here Pixi will only include the minimal required environment variables for your platform to run the command in. The environment will contain all variables set by the conda environment like `"CONDA_PREFIX"`. It will however include some default values from the shell, like: `"DISPLAY"`, `"LC_ALL"`, `"LC_TIME"`, `"LC_NUMERIC"`, `"LC_MEASUREMENT"`, `"SHELL"`, `"USER"`, `"USERNAME"`, `"LOGNAME"`, `"HOME"`, `"HOSTNAME"`,`"TMPDIR"`, `"XPC_SERVICE_NAME"`, `"XPC_FLAGS"`

```toml
[tasks]
clean_command = { cmd = "python run_in_isolated_env.py", clean-env = true }

```

This setting can also be set from the command line with `pixi run --clean-env TASK_NAME`.

`clean-env` not supported on Windows

On Windows it's hard to create a "clean environment" as `conda-forge` doesn't ship Windows compilers and Windows needs a lot of base variables. Making this feature not worthy of implementing as the amount of edge cases will make it unusable.

## Our task runner: deno_task_shell

To support the different OS's (Windows, OSX and Linux), Pixi integrates a shell that can run on all of them. This is [`deno_task_shell`](https://deno.land/manual@v1.35.0/tools/task_runner#built-in-commands). The task shell is a limited implementation of a bourne-shell interface. Task command lines and the values of `tasks.<name>.env` are parsed and expanded by this shell.

### Built-in commands

Next to running actual executable like `./myprogram`, `cmake` or `python` the shell has some built-in commands.

- `cp`: Copies files.
- `mv`: Moves files.
- `rm`: Remove files or directories. Ex: `rm -rf [FILE]...` - Commonly used to recursively delete files or directories.
- `mkdir`: Makes directories. Ex. `mkdir -p DIRECTORY...` - Commonly used to make a directory and all its parents with no error if it exists.
- `pwd`: Prints the name of the current/working directory.
- `sleep`: Delays for a specified amount of time. Ex. `sleep 1` to sleep for 1 second, `sleep 0.5` to sleep for half a second, or `sleep 1m` to sleep a minute
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
- **Negate exit code:** `!` before any command will negate the exit code from 1 to 0 or visa-versa.
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
