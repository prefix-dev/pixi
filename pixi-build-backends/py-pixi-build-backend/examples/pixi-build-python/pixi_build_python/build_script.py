"""
Build script generation for Python backend.
"""

from enum import Enum
from pathlib import Path
from typing import Any, Dict, List
import platform


class Installer(Enum):
    """Available Python installers."""

    UV = "uv"
    PIP = "pip"

    def package_name(self) -> str:
        """Get the package name for this installer."""
        return self.value

    @classmethod
    def determine_installer(cls, dependencies: Dict[str, Any]) -> "Installer":
        """Determine which installer to use based on dependencies."""
        if "uv" in dependencies:
            return cls.UV
        return cls.PIP


class BuildPlatform(Enum):
    """Build platform types."""

    WINDOWS = "windows"
    UNIX = "unix"

    @classmethod
    def current(cls) -> "BuildPlatform":
        """Get current build platform."""
        return cls.WINDOWS if platform.system() == "Windows" else cls.UNIX


class BuildScriptContext:
    """Context for build script generation."""

    def __init__(
        self,
        installer: Installer,
        build_platform: BuildPlatform,
        editable: bool,
        manifest_root: Path,
    ):
        self.installer = installer
        self.build_platform = build_platform
        self.editable = editable
        self.manifest_root = manifest_root

    def render(self) -> List[str]:
        """Render the build script."""
        if self.build_platform == BuildPlatform.WINDOWS:
            python_var = "%PYTHON%"
            src_dir = str(self.manifest_root) if self.editable else "%SRC_DIR%"
        else:
            python_var = "$PYTHON"
            src_dir = str(self.manifest_root) if self.editable else "$SRC_DIR"

        editable_option = " --editable" if self.editable else ""
        common_options = f"-vv --no-deps --no-build-isolation{editable_option}"

        if self.installer == Installer.UV:
            command = f'uv pip install --python "{python_var}" {common_options} "{src_dir}"'
        else:
            command = f'"{python_var}" -m pip install --ignore-installed {common_options} "{src_dir}"'

        lines = [command]

        if self.build_platform == BuildPlatform.WINDOWS:
            lines.append("if errorlevel 1 exit 1")

        return lines
