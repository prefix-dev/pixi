__version__ = "1.0.0"

import sys
from pathlib import Path
import site


def is_editable() -> bool:
    package_name = "editable_pyproject"
    for site_package in site.getsitepackages():
        egg_link_path = Path(site_package).joinpath(f"_{package_name}.pth")
        if egg_link_path.is_file():
            return True
    return False


def check_editable() -> None:
    if is_editable():
        print("The package is installed as editable.")
        sys.exit(0)
    else:
        print("The package is not installed as editable.")
        sys.exit(1)
