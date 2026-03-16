[pixi-skills](https://github.com/pavelzw/pixi-skills) manages and installs coding agent skills across multiple LLM backends.

It discovers skills packaged in pixi environments and installs them into the configuration directories of various coding agents via symlinks.

For a detailed explanation of the motivation and design behind pixi-skills, see the blog post [Managing Agent Skills with Your Package Manager](https://pavel.pink/blog/pixi-skills).

## Installation

```bash
pixi global install pixi-skills
```

## Concepts

### Skills

A skill is a directory containing a `SKILL.md` file with YAML frontmatter:

```markdown
---
name: my-skill
description: "Does something useful for the agent"
---

Skill instructions go here as Markdown.
The agent reads this file to understand what the skill does.
```

The `name` field is optional and defaults to the directory name.
The `description` field is required.

### skill-forge

A collection of ready-to-use skills is available as conda packages on the [skill-forge](https://prefix.dev/channels/skill-forge) channel ([source](https://github.com/pavelzw/skill-forge)).

To use skills from skill-forge, add the channel and the desired skill packages to your `pixi.toml`:

```toml
[workspace]
channels = ["conda-forge", "https://prefix.dev/skill-forge"]

[dependencies]
polars = ">=1,<2"

[feature.dev.dependencies]
agent-skill-polars = "*"
```

### Scopes

- **Local** skills are discovered from the current project's pixi environment at `.pixi/envs/<env>/share/agent-skills/`.
- **Global** skills are discovered from globally installed pixi packages at `~/.pixi/envs/agent-skill-*/share/agent-skills/`.

Skills are installed as relative symlinks for portability.

## Usage

### Manage skills interactively

```bash
# Interactive mode - prompts for backend and scope
pixi skills manage

# Specify backend and scope directly
pixi skills manage --backend claude --scope local
```

This opens an interactive checkbox selector where you can choose which skills to install or uninstall.

### List available skills

```bash
# List all local and global skills
pixi skills list

# List only local skills
pixi skills list --scope local

# List skills from a specific pixi environment
pixi skills list --env myenv
```

### Show installed skills

```bash
# Show installed skills across all backends
pixi skills status

# Show installed skills for a specific backend
pixi skills status --backend claude
```
