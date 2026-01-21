import os
from pathlib import Path
from typing import Any, Optional, Dict, List
import re
import toml


def parse_entry_points_from_scripts(scripts: Dict[str, str]) -> List[str]:
    """
    Parse entry points from a dictionary of scripts.

    This function validates entry point format and converts a scripts dictionary
    to a list of properly formatted entry point strings.

    Parameters
    ----------
    scripts : Dict[str, str]
        Dictionary mapping script names to entry points (e.g., {"script": "module:function"})

    Returns
    -------
    List[str]
        List of formatted entry point strings (e.g., ["script = module:function"])

    Raises
    ------
    ValueError
        If any entry point has invalid format

    Examples
    --------
    >>> scripts = {"my_script": "mymodule:main", "other": "pkg.mod:func"}
    >>> parse_entry_points_from_scripts(scripts)
    ['my_script = mymodule:main', 'other = pkg.mod:func']
    """
    if not scripts:
        return []

    entry_points = []
    # Entry point format validation regex: name should be valid Python identifier-like,
    # and entry point should be in format "module:function" or "module.submodule:function"
    entry_point_pattern = re.compile(r"^[a-zA-Z_][a-zA-Z0-9_]*(\.[a-zA-Z_][a-zA-Z0-9_]*)*:[a-zA-Z_][a-zA-Z0-9_]*$")

    for name, entry_point in scripts.items():
        # Validate entry point format
        if not entry_point_pattern.match(entry_point):
            raise ValueError(f"Invalid entry point format: {entry_point}")

        # Format as "name = entry_point"
        formatted_entry_point = f"{name} = {entry_point}"
        entry_points.append(formatted_entry_point)

    return entry_points


def extract_entry_points(pyproject_manifest: Optional[Dict[str, Any]]) -> List[str]:
    """
    Extract entry points from pyproject.toml.

    Parameters
    ----------
    pyproject_manifest : Optional[dict]
        The pyproject.toml manifest dictionary.

    Returns
    -------
    list
        A list of entry points, or empty list if no scripts found.

    Examples
    --------
    ```python
    >>> manifest = {"project": {"scripts": {"my_script": "module:function"}}}
    >>> extract_entry_points(manifest)
    ['my_script = module:function']
    >>> extract_entry_points(None)
    []
    >>> extract_entry_points({})
    []
    >>>
    ```
    """
    if not pyproject_manifest:
        return []

    project = pyproject_manifest.get("project", {})
    scripts: Dict[str, str] = project.get("scripts", {})

    if not scripts:
        return []

    # Use the Python implementation instead of FFI
    return parse_entry_points_from_scripts(scripts)


def read_pyproject_toml(manifest_root: Path) -> Optional[Dict[str, Any]]:
    """Read pyproject.toml if it exists."""
    pyproject_path = manifest_root / "pyproject.toml"
    if pyproject_path.exists():
        return toml.load(pyproject_path)
    return None


def get_build_input_globs(config: Any, workdir: Path, editable: bool) -> List[str]:
    """Get build input globs for Python package."""
    base_globs = [
        # Source files
        "**/*.c",
        "**/*.cpp",
        "**/*.rs",
        "**/*.sh",
        # Common data files
        "**/*.json",
        "**/*.yaml",
        "**/*.yml",
        "**/*.txt",
        # Project configuration
        "setup.py",
        "setup.cfg",
        "pyproject.toml",
        "requirements*.txt",
        "Pipfile",
        "Pipfile.lock",
        "poetry.lock",
        "tox.ini",
        # Build configuration
        "Makefile",
        "MANIFEST.in",
        "tests/**/*.py",
        "docs/**/*.rst",
        "docs/**/*.md",
        # Versioning
        "VERSION",
        "version.py",
    ]

    python_globs = [] if editable else ["**/*.py", "**/*.pyx"]

    all_globs = base_globs + python_globs
    if hasattr(config, "extra_input_globs"):
        all_globs.extend(config.extra_input_globs)

    return all_globs


def get_editable_setting(python_params: Any) -> bool:
    """Get editable setting from environment or params."""
    env_editable = os.environ.get("BUILD_EDITABLE_PYTHON", "").lower() == "true"
    if env_editable:
        return True

    if python_params and hasattr(python_params, "editable"):
        return bool(python_params.editable)

    return False
