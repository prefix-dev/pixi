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
