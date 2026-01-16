#!/usr/bin/env python
# /// conda-script
# [dependencies]
# python = "3.12.*"
# [script]
# channels = ["conda-forge"]
# entrypoint = "python"
# /// end-conda-script

"""
A simple Hello World script demonstrating conda-script metadata.

Run with: pixi exec hello_python.py
"""

import sys
import platform

print("=" * 60)
print("Hello from Python with conda-script!")
print("=" * 60)
print(f"Python version: {sys.version}")
print(f"Platform: {platform.system()} {platform.machine()}")
print(f"Executable: {sys.executable}")
print("=" * 60)
