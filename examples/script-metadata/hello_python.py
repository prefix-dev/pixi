#!/usr/bin/env python
# /// script
# requires-python = ">=3.12"
# ///

"""
A minimal Hello World script with inline metadata.

`requires-python` is turned into a conda `python` dependency, the packages
come from the default channels (conda-forge unless configured otherwise),
and `.py` scripts that require Python are run with `python` automatically.

Run with: pixi exec hello_python.py
"""

import platform
import sys

print("=" * 60)
print("Hello from Python with inline script metadata!")
print("=" * 60)
print(f"Python version: {sys.version}")
print(f"Platform: {platform.system()} {platform.machine()}")
print(f"Executable: {sys.executable}")
print("=" * 60)
