Every Pixi workspace is described by a Pixi manifest.
In this simple example we have a single task `start` which runs a Python file and two dependencies, `cowpy` and `python`.

```toml title="pixi.toml"
--8<-- "docs/source_files/pixi_workspaces/introduction/task_add/pixi.toml"
```

`channels` describes where our dependencies come from and `platforms` which platforms we support.
However, you might wonder why we need to specify the platforms if Pixi could just extract this information from your operating system.
That is because every dependency in your environment is stored in the lockfile called `pixi.lock`.
This ensures that even if you run your workspace on a different platform, the environment will contain exactly the dependencies that were solved on your machine.
This is one of the core features that makes Pixi reproducible.
Learn more about lock files in [this chapter](./workspace/lockfile.md).


## Multiple environments

We already have a quite powerful setup which is sufficient for many use cases.
However, certain things are hard to do with the way things are set up right now.
What if I wanted to check if my script works with multiple versions of Python?
There cannot be multiple versions of the same package in one environment.
Luckily, Pixi is able to manage multiple environments!

Environments are composed of features, so let's create a `py312` and `py313` features each with `python` set to a different version.
Then we will add those features to environments of the same name.

```toml title="pixi.toml" hl_lines="12-20"
--8<-- "docs/source_files/pixi_workspaces/introduction/multi_env/pixi.toml"
```

Pixi does two things behind the scenes which might not be immediately obvious.
First, it automatically creates both a feature and environment called `default`.
`[dependencies]` and `[tasks]` belong to that feature.
Second, it adds the `default` feature to each environment unless you explicitly opt-out.
That means you can read the manifest as if it were declared like this:

```toml hl_lines="6 9 19 20 21"
[workspace]
channels = ["conda-forge"]
name = "hello-world"
platforms = ["linux-64", "osx-arm64", "win-64"]

[feature.default.tasks]
start = 'python hello.py'

[feature.default.dependencies]
cowpy = "1.1.*"

[feature.py312.dependencies]
python = "3.12.*"

[feature.py313.dependencies]
python = "3.13.*"

[environments]
default = ["default"]
py312 = ["default", "py312"]
py313 = ["default", "py313"]
```

Let's adapt the Python script so that it displays the current Python version:

```py title="hello.py"
--8<-- "docs/source_files/pixi_workspaces/introduction/multi_env/hello.py"
```

The task `start` is available in both `py312` and `py313`, so we can test the script like this to test against Python 3.12:

```bash
pixi run --environment=py312 start
```

```
 _________________________
< Hello from Python 3.12! >
 -------------------------
     \   ^__^
      \  (oo)\_______
         (__)\       )\/\
           ||----w |
           ||     ||
```

And we can run this command to try it with Python 3.13:


```bash
pixi run --environment=py312 start
```

```
 _________________________
< Hello from Python 3.12! >
 -------------------------
     \   ^__^
      \  (oo)\_______
         (__)\       )\/\
           ||----w |
           ||     ||
```


## Going further

There is still much more that Pixi has to offer.
Check out the topics on the sidebar on the left to learn more.

And don't forget to [join our Discord](https://discord.gg/kKV8ZxyzY4) to join our community of Pixi enthusiasts!
