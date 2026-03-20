"""Build setup that compiles Zig source into a shared library.

In a conda-build environment, the zig compiler is installed with a
platform-prefixed name (e.g. ``arm64-apple-darwin20.0.0-zig``) under
``$BUILD_PREFIX/bin``.  The activation script sets ``CONDA_ZIG_HOST``
to the correct binary name, so we look there first and fall back to
a plain ``zig`` for local development.
"""

import os
import shutil
import subprocess
import sys
from pathlib import Path

from setuptools import setup
from setuptools.command.build_py import build_py

LIB_NAME = {
    "darwin": "libmathlib.dylib",
    "linux": "libmathlib.so",
    "win32": "mathlib.dll",
}[sys.platform]


def _find_zig():
    """Return the path to the zig binary."""
    # The conda ``zig`` package sets CONDA_ZIG_HOST during activation.
    zig_name = os.environ.get("CONDA_ZIG_HOST") or os.environ.get("CONDA_ZIG_BUILD")
    if zig_name:
        candidate = Path(os.environ.get("BUILD_PREFIX", "")) / "bin" / zig_name
        if candidate.exists():
            return str(candidate)

    # Fall back to whatever is on PATH (local dev with ``zig`` installed).
    return shutil.which("zig") or "zig"


class BuildZig(build_py):
    """Compile the Zig shared library, then run the normal ``build_py``."""

    def run(self):
        src = Path("zig_python_example/mathlib.zig").resolve()
        out = Path("zig_python_example") / LIB_NAME

        subprocess.check_call([
            _find_zig(), "build-lib", str(src),
            "-dynamic", "-O", "ReleaseSafe",
            f"-femit-bin={out}",
        ])

        super().run()

        # Copy the library into the wheel staging area.
        if self.build_lib:
            dest = Path(self.build_lib) / "zig_python_example" / LIB_NAME
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(out, dest)


setup(
    cmdclass={"build_py": BuildZig},
)
