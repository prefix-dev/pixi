--8<-- [start:example]
## Examples

```shell
pixi run python
pixi run cowpy "Hey pixi user"
pixi run --manifest-path ~/myworkspace/pixi.toml python
pixi run --frozen python
pixi run --locked python
# If you have specified a custom task in the pixi.toml you can run it with run as well
pixi run build
# Extra arguments will be passed to the tasks command.
pixi run task argument1 argument2
# Skip dependencies of the task
pixi run --skip-deps task
# Run in dry-run mode to see the commands that would be run
pixi run --dry-run task

# If you have multiple environments you can select the right one with the --environment flag.
pixi run --environment cuda python

# THIS DOESN'T WORK ON WINDOWS
# If you want to run a command in a clean environment you can use the --clean-env flag.
# The PATH should only contain the pixi environment here.
pixi run --clean-env "echo \$PATH"

```

## Notes
!!! info
    In `pixi` the [`deno_task_shell`](https://deno.land/manual@v1.35.0/tools/task_runner#task-runner) is the underlying runner of the run command.
    Checkout their [documentation](https://deno.land/manual@v1.35.0/tools/task_runner#task-runner) for the syntax and available commands.
    This is done so that the run commands can be run across all platforms.

!!! tip "Cross environment tasks"
    If you're using the `depends-on` feature of the `tasks`, the tasks will be run in the order you specified them.
    The `depends-on` can be used cross environment, e.g. you have this `pixi.toml`:
    ??? "pixi.toml"
        ```toml
        [tasks]
        start = { cmd = "python start.py", depends-on = ["build"] }

        [feature.build.tasks]
        build = "cargo build"
        [feature.build.dependencies]
        rust = ">=1.74"

        [environments]
        build = ["build"]
        ```

        Then you're able to run the `build` from the `build` environment and `start` from the default environment.
        By only calling:
        ```shell
        pixi run start
        ```
--8<-- [end:example]
