__version__ = "1.0.0"

import sys
import os
import site


def is_editable() -> bool:
    package_name = "editable-pyproject"  # Replace with your package name
    for site_package in site.getsitepackages():
        egg_link_path = os.path.join(site_package, f"{package_name}.egg-link")
        if os.path.isfile(egg_link_path):
            return True
    return False


def check_editable() -> None:
    if is_editable():
        print("The package is installed as editable.")
        sys.exit(0)
    else:
        print("The package is not installed as editable.")
        sys.exit(1)
