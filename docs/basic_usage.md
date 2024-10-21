---
part: pixi
title: Basic usage
description: Taking your first steps with pixi
---
Ensure you've got `pixi` set up. If running `pixi` doesn't show the help, see the [getting started](index.md) if it doesn't.

```shell
pixi
```

Initialize a new project and navigate to the project directory.

```shell
pixi init pixi-hello-world
cd pixi-hello-world
```

Add the dependencies you would like to use.

```shell
pixi add python
```

Create a file named `hello_world.py` in the directory and paste the following code into the file.

```py title="hello_world.py"
def hello():
    print("Hello World, to the new revolution in package management.")

if __name__ == "__main__":
    hello()
```

Run the code inside the environment.

```shell
pixi run python hello_world.py
```

You can also put this run command in a **task**.

```shell
pixi task add hello python hello_world.py
```

After adding the task, you can run the task using its name.

```shell
pixi run hello
```

Use the `shell` command to activate the environment and start a new shell in there.

```shell
pixi shell
python
exit()
```

You've just learned the basic features of pixi:

1. initializing a project
2. adding a dependency.
2. adding a task, and executing it.
3. running a program.

Feel free to play around with what you just learned like adding more tasks, dependencies or code.

Happy coding!

## Use pixi as a global installation tool

Use pixi to install tools on your machine.

Some notable examples:

```shell
# Awesome cross shell prompt, huge tip when using pixi!
pixi global install starship

# Want to try a different shell?
pixi global install fish

# Install other prefix.dev tools
pixi global install rattler-build

# Install a multi package environment
pixi global install --environment data-science-env --expose python --expose jupyter python jupyter numpy pandas
```

## Use pixi in GitHub Actions

You can use pixi in GitHub Actions to install dependencies and run commands.
It supports automatic caching of your environments.

```yml
- uses: prefix-dev/setup-pixi@v0.5.1
- run: pixi run cowpy "Thanks for using pixi"
```

See the [GitHub Actions](./advanced/github_actions.md) for more details.
