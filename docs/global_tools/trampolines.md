## Trampolines

To increase efficiency, `pixi` uses *trampolines*—small, specialized binary files that manage configuration and environment setup before executing the main binary. The trampoline approach allows for skipping the execution of activation scripts that have a significant performance impact.

When you execute a globally installed executable, a trampoline performs the following sequence of steps:

* Each trampoline first reads a configuration file named after the binary being executed. This configuration file, in JSON format (e.g., `python.json`), contains key information about how the environment should be set up. The configuration file is stored in [`$PIXI_HOME`](../reference/environment_variables.md)`/bin/trampoline_configuration`.
* Once the configuration is loaded and the environment is set, the trampoline executes the original binary with the correct environment settings.
* When installing a new binary, a new trampoline is placed in the [`$PIXI_HOME`](../reference/environment_variables.md)`/bin` directory and is hard-linked to the [`$PIXI_HOME`](../reference/environment_variables.md)`/bin/trampoline_configuration/trampoline_bin`. This optimizes storage space and avoids duplication of the same trampoline.

The trampoline will take care that the `PATH` contains the newest changes on your local `PATH` while avoiding caching temporary `PATH` changes during installation.
If you want to control the base `PATH` pixi considers, you can set `export PIXI_BASE_PATH=$PATH` in your shell startup script.
