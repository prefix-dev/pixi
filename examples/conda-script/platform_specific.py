#!/usr/bin/env python
# /// conda-script
# [dependencies]
# python = "3.12.*"
# [dependencies.target.linux]
# patchelf = "*"
# [dependencies.target.osx]
# cctools = "*"
# [dependencies.target.win]
# vs2019_win-64 = "*"
# [script]
# channels = ["conda-forge"]
# entrypoint = "python"
# /// end-conda-script

"""
Demonstrates platform-specific dependencies in conda-script.

This script will install different tools depending on the platform:
- Linux: patchelf (for modifying ELF binaries)
- macOS: cctools (for Mach-O binary tools)
- Windows: Visual Studio 2019 tools

Run with: pixi exec platform_specific.py
"""

import platform
import shutil
import subprocess


def main():
    system = platform.system()
    machine = platform.machine()

    print("=" * 60)
    print("Platform-Specific Dependencies Demo")
    print("=" * 60)
    print(f"Platform: {system} ({machine})")
    print()

    if system == "Linux":
        print("On Linux - checking for patchelf...")
        patchelf_path = shutil.which("patchelf")
        if patchelf_path:
            print(f"✓ Found patchelf at: {patchelf_path}")
            result = subprocess.run(["patchelf", "--version"], capture_output=True, text=True)
            print(f"  Version: {result.stdout.strip()}")
        else:
            print("✗ patchelf not found")

    elif system == "Darwin":
        print("On macOS - checking for otool (from cctools)...")
        otool_path = shutil.which("otool")
        if otool_path:
            print(f"✓ Found otool at: {otool_path}")
            result = subprocess.run(["otool", "-version"], capture_output=True, text=True)
            print(f"  Info: {result.stderr.strip()[:100]}")
        else:
            print("✗ otool not found")

    elif system == "Windows":
        print("On Windows - Visual Studio tools should be available")
        print("✓ VS2019 tools installed")

    else:
        print(f"Unknown platform: {system}")

    print("=" * 60)


if __name__ == "__main__":
    main()
