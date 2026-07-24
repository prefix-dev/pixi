"""Build script for ament_python packages (setup.py and pyproject.toml).

Runs under rattler-build's `python` interpreter, which activates the build
environment first, so PREFIX/LIBRARY_PREFIX and friends are available as
environment variables. A single Python implementation replaces the previous
shell + batch templates: the entry points are read from the installed package
metadata, which is both more robust and platform independent.
"""

import importlib
import importlib.metadata
import os
import shutil
import subprocess
import sys
import sysconfig
from pathlib import Path

# Substituted by render_build_script.
SRC_DIR = Path(r"@SRC_DIR@")
PKG_NAME = "@ROS_PKG_NAME@"

# On Windows the ROS files live under %LIBRARY_PREFIX%; elsewhere under $PREFIX.
if os.name == "nt":
    ROS_PREFIX = Path(os.environ["LIBRARY_PREFIX"])
else:
    ROS_PREFIX = Path(os.environ["PREFIX"])


def install():
    """Build and install the package with the host environment's backend.

    --no-build-isolation uses the backend from the host environment (no network
    at build time). It works for setup.py packages too: setuptools is always a
    host dependency for ament_python.
    """
    subprocess.check_call(
        [sys.executable, "-m", "pip", "install", ".",
         "--no-deps", "--no-build-isolation", "-vvv"],
        cwd=SRC_DIR,
    )


def register_ament_index():
    """Register the package in the ament resource index.

    setup.py packages that use data_files already do this during install;
    repeating it is harmless and it is the only way pyproject-only packages can
    be registered.
    """
    packages_index = ROS_PREFIX / "share" / "ament_index" / "resource_index" / "packages"
    packages_index.mkdir(parents=True, exist_ok=True)
    (packages_index / PKG_NAME).touch()

    share_pkg = ROS_PREFIX / "share" / PKG_NAME
    share_pkg.mkdir(parents=True, exist_ok=True)
    package_xml = SRC_DIR / "package.xml"
    if package_xml.is_file():
        shutil.copy2(package_xml, share_pkg / "package.xml")


def install_entry_points():
    """Copy console scripts into lib/<pkg> where `ros2 run` looks for them.

    pip installs them into the scripts directory (bin / Scripts); the names come
    from the installed metadata, so this works regardless of whether they were
    declared in setup.py, setup.cfg or pyproject.toml.
    """
    importlib.invalidate_caches()
    try:
        dist = importlib.metadata.distribution(PKG_NAME)
    except importlib.metadata.PackageNotFoundError:
        return

    names = [
        ep.name
        for ep in dist.entry_points
        if ep.group in ("console_scripts", "gui_scripts")
    ]
    if not names:
        return

    scripts_dir = Path(sysconfig.get_path("scripts"))
    lib_dir = ROS_PREFIX / "lib" / PKG_NAME
    lib_dir.mkdir(parents=True, exist_ok=True)
    for name in names:
        # On Windows a console script is `name.exe` (and sometimes a
        # `name-script.py` wrapper); on other platforms it is just `name`.
        matches = scripts_dir.glob(name + "*") if os.name == "nt" else [scripts_dir / name]
        for src in matches:
            if src.is_file():
                shutil.copy2(src, lib_dir / src.name)


def main():
    install()
    register_ament_index()
    install_entry_points()


if __name__ == "__main__":
    main()
