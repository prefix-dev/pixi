---
part: pixi/advanced
title: Advanced tasks
description: Learn how to interact with pixi tasks
---

When building a package, you often have to do more than just run the code.
Steps like formatting, linting, compiling, testing, benchmarking, etc. are often part of a project.
With pixi tasks, this should become much easier to do.

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

```shell
pixi run start
```

The tasks will be executed after each other:

- First `configure` because it has no dependencies.
- Then `build` as it only depends on `configure`.
- Then `start` as all it dependencies are run.

If one of the commands fails (exit with non-zero code.) it will stop and the next one will not be started.

With this logic, you can also create aliases as you don't have to specify any command in a task.

```shell
pixi task add fmt ruff
pixi task add lint pylint
```

```shell
pixi task alias style fmt lint
```

Results in the following `pixi.toml`.

```toml title="pixi.toml"
fmt = "ruff"
lint = "pylint"
style = { depends-on = ["fmt", "lint"] }
```

Now run both tools with one command.

```shell
pixi run style
```

## Working directory

Pixi tasks support the definition of a working directory.

`cwd`" stands for Current Working Directory.
The directory is relative to the pixi package root, where the `pixi.toml` file is located.

Consider a pixi project structured as follows:

```shell
├── pixi.toml
└── scripts
    └── bar.py
```

To add a task to run the `bar.py` file, use:

```shell
pixi task add bar "python bar.py" --cwd scripts
```

This will add the following line to [manifest file](../reference/pixi_manifest.md):

```toml title="pixi.toml"
[tasks]
bar = { cmd = "python bar.py", cwd = "scripts" }
```

## Caching

When you specify `inputs` and/or `outputs` to a task, pixi will reuse the result of the task.

For the cache, pixi checks that the following are true:

- No package in the environment has changed.
- The selected inputs and outputs are the same as the last time the task was
  run. We compute fingerprints of all the files selected by the globs and
  compare them to the last time the task was run.
- The command is the same as the last time the task was run.

If all of these conditions are met, pixi will not run the task again and instead use the existing result.

Inputs and outputs can be specified as globs, which will be expanded to all matching files.

```toml title="pixi.toml"
[tasks]
# This task will only run if the `main.py` file has changed.
run = { cmd = "python main.py", inputs = ["main.py"] }

# This task will remember the result of the `curl` command and not run it again if the file `data.csv` already exists.
download_data = { cmd = "curl -o data.csv https://example.com/data.csv", outputs = ["data.csv"] }

# This task will only run if the `src` directory has changed and will remember the result of the `make` command.
build = { cmd = "make", inputs = ["src/*.cpp", "include/*.hpp"], outputs = ["build/app.exe"] }
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
You can make sure the environment of a task is "pixi only".
Here pixi will only include the minimal required environment variables for your platform to run the command in.
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

To support the different OS's (Windows, OSX and Linux), pixi integrates a shell that can run on all of them.
This is [`deno_task_shell`](https://deno.land/manual@v1.35.0/tools/task_runner#built-in-commands).
The task shell is a limited implementation of a bourne-shell interface.

### Built-in commands

Next to running actual executable like `./myprogram`, `cmake` or `python` the shell has some built-in commandos.

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
- **Shell variables:** Shell variables are similar to environment variables, but won’t be exported to spawned commands.
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
