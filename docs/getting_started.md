# Getting Started


Every Pixi workspace is described by a Pixi manifest.
In this simple example we have a single task `start` which runs a Python file and two dependencies, `cowpy` and `python`.

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/introduction/task_add/pixi.toml"
```

`channels` describes where our dependencies come from and `platforms` which platforms we support.
However, you might wonder why we need to specify the platforms if Pixi could just extract this information from your operating system.
That is because every dependency in your environment is stored in the lockfile called `pixi.lock`.
This ensures that even if you run your workspace on a different platform, the environment will contain exactly the dependencies that were solved on your machine.
This is one of the core features that makes pixi reproducible.
Learn more about lock files in [this chapter](./environments/lockfile.md).
