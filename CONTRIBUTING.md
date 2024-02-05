# Contributing to Pixi
## Introduction
Thank you for your interest in contributing to Pixi! We value the contributions from our community and look forward to collaborating with you.

## Code of Conduct
Before contributing, please read our [Code of Conduct](https://github.com/prefix-dev/pixi/blob/main/CODE_OF_CONDUCT.md). We expect all our contributors to follow it to ensure a welcoming and inclusive environment.

## Getting Started
**Familiarize yourself with Pixi:** Understand its functionality and codebase.
**Set up your environment:** Ensure you have the necessary tools and dependencies.

## How to Contribute
- **Find an Issue:** Look for open issues or create a new issue to discuss your idea.
- **Fork the Repository:** Make a copy of the repository to your account.
- **Create a Branch:** Create a branch in your fork to work on your changes.
- **Develop and Test:** Implement your changes and make sure they are thoroughly tested.
- **Write Clear Commit Messages:** Make your changes easy to understand.
- **Create a Pull Request:** Submit your changes for review.

> [!TIP]
> Discuss your change in the issue before developing the solution when the design is not predefined.
> This will help speedup the actual pull request review.

## Pull Request Process
- Ensure your code adheres to the project's coding standards, we use automated tooling for that, try `pixi run lint`.
- Document new code using the Rust docstrings like we do in most places.
- Update the `README.md` or documentation (`docs/`) with details of changes to the interface, if applicable.
- Your pull request will be reviewed by maintainers, who may suggest changes.

## Reporting Bugs
- Use the issue tracker to report bugs.
- Clearly describe the issue, including steps to reproduce the bug.
- Include any relevant logs or error messages.

## Feature Requests
- Use the issue tracker to suggest new features.
- Explain how the feature would be beneficial to the project.

## Questions and Discussions
For general questions, consider using our chat platform: [Discord](https://discord.gg/kKV8ZxyzY4)

## Acknowledgments
Your contributions are highly appreciated and will be credited accordingly.

## License
By contributing to Pixi, you agree that your contributions will be licensed under its [LICENSE](https://github.com/prefix-dev/pixi/blob/main/LICENSE).

# Tips while developing on `pixi`

## Pixi is a pixi project so use a preinstalled `pixi` to run the predefined tasks
```shell
pixi run build
pixi run lint
pixi run test
pixi run test-all
pixi run install # only works on unix systems as on windows you can't overwrite the binary while it's running
```

## Get your code ready for a PR
We use [`pre-commit`](https://pre-commit.com/) to run all the formatters and linters that we use.
If you have `pre-commit` installed on your system you can run `pre-commit install` to run the tools before you commit or push.
If you don't have it on your system either use `pixi global install pre-commit` or use the one in your environment.
```shell
pixi run lint
```

When you commit your code, please try to come up with a good commit message.
The maintainers (try to) use [conventional-commits](https://www.conventionalcommits.org/en/v1.0.0/).
```shell
git add FILES_YOU_CHANGED
# This is the conventional commit convention:
git commit -m "<type>[optional scope]: <description>"
# An example:
git commit -m "feat: add xxx to the pixi.toml"
```

## Color decisions in the ui code
We use the `console::style` function to colorize the output of the ui.
```rust
use console::style;
println!("{} {}", style("Hello").green(), style("world").red());
```

To sync the colors of the different parts of the ui, we use the following rules:
- `style("environment").magenta()`: The environment name
- `style("feature").cyan()`: The feature name
- `style("task").blue()`: The task name

These styles are put in the `consts` module. If you want to add a new generic color, please add it there.
