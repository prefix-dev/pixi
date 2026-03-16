"""
Build script generation for Python backend.
"""

from enum import Enum
from pathlib import Path
import platform
from catkin_pkg.package import Package as CatkinPackage
from importlib.resources import files

from pixi_build_ros.distro import Distro


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
        script_content: str,
        build_platform: BuildPlatform,
        source_dir: Path,
    ):
        self.script_content = script_content
        self.build_platform = build_platform
        self.source_dir = source_dir

    def render(self) -> list[str]:
        """Render the build script content into a list of lines."""
        return self.script_content.splitlines()

    @classmethod
    def load_from_template(
        cls, pkg: CatkinPackage, platform: BuildPlatform, source_dir: Path, distro: Distro
    ) -> "BuildScriptContext":
        """Get the build script from the template directory based on the package type."""
        # TODO: deal with other script languages, e.g. for Windows
        if pkg.get_build_type() in ["ament_cmake"]:
            template_name = "build_ament_cmake.sh" if platform == BuildPlatform.UNIX else "bld_ament_cmake.bat"
        elif pkg.get_build_type() in ["ament_python"]:
            template_name = "build_ament_python.sh" if platform == BuildPlatform.UNIX else "bld_ament_python.bat"
        elif pkg.get_build_type() in ["cmake", "catkin"]:
            template_name = "build_catkin.sh" if platform == BuildPlatform.UNIX else "bld_catkin.bat"
        else:
            raise ValueError(f"Unsupported build type: {pkg.get_build_type()}")

        script_content = ""
        try:
            # Try to load from installed package data first
            templates_pkg = files("pixi_build_ros") / "templates"
            template_file = templates_pkg / template_name
            script_content = template_file.read_text()
        except (ImportError, FileNotFoundError):
            # Fallback to the development path
            templates_pkg = Path(__file__).parent.parent.parent / "templates"
            script_path = templates_pkg / template_name
            with open(script_path) as f:
                script_content = f.read()

        script_content = (
            script_content.replace("@SRC_DIR@", str(source_dir))
            .replace("@DISTRO@", distro.name)
            .replace("@BUILD_DIR@", "build")
            .replace("@BUILD_TYPE@", "Release")
        )

        return cls(
            script_content=script_content,
            build_platform=platform,
            source_dir=source_dir,
        )
