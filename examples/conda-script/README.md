# Conda Script Examples

This directory contains examples demonstrating the conda-script metadata feature in Pixi.

## What is Conda Script?

Conda script allows you to embed conda environment metadata directly in script files using specially formatted comment blocks. This makes scripts self-contained and easy to share.

## Examples

### 1. `hello_python.py` - Basic Example

A simple Hello World script that shows the basics:
- Python version pinning
- Basic metadata structure
- How to run scripts with `pixi exec`

```bash
pixi exec hello_python.py
```

### 2. `web_request.py` - External Dependencies

Demonstrates using external packages (requests) from conda-forge:
- Adding package dependencies
- Using installed packages in your script

```bash
pixi exec web_request.py
```

### 3. `platform_specific.py` - Platform-Specific Configuration

Shows how to specify different dependencies for different platforms:
- Linux: installs `patchelf`
- macOS: installs `cctools`
- Windows: installs Visual Studio tools

```bash
pixi exec platform_specific.py
```

### 4. Multi-Language Examples

The following examples demonstrate conda-script support across different programming languages:

#### Bash (`hello_bash.sh`)
Shell scripting with conda-managed tools:
```bash
pixi exec examples/conda-script/hello_bash.sh
```

#### R (`hello_r.R`)
R script with conda-managed R environment:
```bash
pixi exec examples/conda-script/hello_r.R
```

#### Julia (`hello_julia.jl`)
Julia script with conda-managed Julia:
```bash
pixi exec examples/conda-script/hello_julia.jl
```

#### Node.js (`hello_node.js`)
JavaScript with conda-managed Node.js:
```bash
pixi exec examples/conda-script/hello_node.js
```

#### Perl (`hello_perl.pl`)
Perl script with conda-managed Perl:
```bash
pixi exec examples/conda-script/hello_perl.pl
```

## How to Use

1. Make sure you have Pixi installed
2. Run any example with: `pixi exec <script_name>`
3. The first run will create an environment and install dependencies
4. Subsequent runs reuse the cached environment

## Learn More

See the [full documentation](../../docs/features/conda_script_metadata.md) for:
- Complete specification
- More examples
- Platform selectors
- Custom entrypoints
- Best practices

## Creating Your Own

To create your own conda-script:

```python
#!/usr/bin/env python
# /// conda-script
# [dependencies]
# python = "3.12.*"
# your-package = "*"
# [script]
# channels = ["conda-forge"]
# entrypoint = "python"
# /// end-conda-script

# Your code here
import your_package
```

Then run with:
```bash
pixi exec your_script.py
```
