![pixi-browse demo](https://raw.githubusercontent.com/pavelzw/pixi-browse/refs/heads/main/.github/assets/demo.gif)

[pixi-browse](https://github.com/pavelzw/pixi-browse) is an interactive terminal UI for browsing conda package metadata.
Explore packages, versions, dependencies, and more from any conda channel, right from your terminal.

## Features

- **Browse packages** from any conda channel (conda-forge, prefix.dev, etc.)
- **Fuzzy search** to quickly filter through thousands of packages
- **Inspect versions** grouped by platform with collapsible sections
- **View detailed metadata** including dependencies, license, checksums, build info, and timestamps
- **Inspect package contents** — file listings and `about.json` extracted directly from artifacts
- **Clickable links** to source repositories, maintainer GitHub profiles, and provenance commits
- **Download artifacts** directly to your working directory
- **Vim-style keybindings** for fast keyboard-driven navigation

## Installation

```bash
pixi global install pixi-browse
```

Or use it without installation:

```bash
pixi exec pixi-browse
```

## Usage

```bash
# Browse conda-forge (default)
pixi browse

# Browse a different channel
pixi browse -c prefix.dev/conda-forge

# Restrict to specific platforms
pixi browse -p linux-64 -p osx-arm64
```
