"""
Python generator implementation using Python bindings.
"""

from dataclasses import dataclass
from pathlib import Path
from typing import Dict, Optional, List, Any
from pixi_build_backend.types.generated_recipe import (
    GenerateRecipeProtocol,
    GeneratedRecipe,
)
from pixi_build_backend.types.intermediate_recipe import NoArchKind, Python, Script
from pixi_build_backend.types.platform import Platform
from pixi_build_backend.types.project_model import ProjectModel
from pixi_build_backend.types.python_params import PythonParams
from pixi_build_backend.types.item import ItemPackageDependency

from .build_script import BuildScriptContext, Installer, BuildPlatform
from .utils import extract_entry_points
from .utils import read_pyproject_toml, get_build_input_globs, get_editable_setting


@dataclass
class PythonBackendConfig:
    """Python backend configuration."""

    noarch: Optional[bool] = None
    env: Optional[Dict[str, str]] = None
    debug_dir: Optional[Path] = None
    extra_input_globs: Optional[List[str]] = None

    def is_noarch(self) -> bool:
        """Whether to build a noarch package or a platform-specific package."""
        return self.noarch is None or self.noarch

    def get_debug_dir(self) -> Optional[Path]:
        """Get debug directory if set."""
        return self.debug_dir


class PythonGenerator(GenerateRecipeProtocol):
    """Python recipe generator using Python bindings."""

    def generate_recipe(
        self,
        model: ProjectModel,
        config: Dict[str, Any],
        manifest_path: str,
        host_platform: Platform,
        python_params: Optional[PythonParams] = None,
        channels: Optional[List[str]] = None,
    ) -> GeneratedRecipe:
        """Generate a recipe for a Python package."""
        backend_config: PythonBackendConfig = PythonBackendConfig(**config)

        manifest_root = Path(manifest_path).parent

        # Create base recipe from model
        generated_recipe = GeneratedRecipe.from_model(model)

        # Get recipe components
        recipe = generated_recipe.recipe
        requirements = recipe.requirements

        # Resolve requirements for the host platform
        resolved_requirements = requirements.resolve(host_platform)

        # Determine installer (pip or uv)
        installer = Installer.determine_installer(resolved_requirements.host)
        installer_name = installer.package_name()

        # Add installer to host requirements if not present
        if installer_name not in resolved_requirements.host:
            requirements.host.append(ItemPackageDependency(installer_name))

        # Add python to both host and run requirements if not present
        if "python" not in resolved_requirements.host:
            requirements.host.append(ItemPackageDependency("python"))
        if "python" not in resolved_requirements.run:
            requirements.run.append(ItemPackageDependency("python"))

        # Determine build platform
        build_platform = BuildPlatform.current()

        # Get editable setting
        editable = get_editable_setting(python_params)

        # Generate build script
        build_script_context = BuildScriptContext(
            installer=installer,
            build_platform=build_platform,
            editable=editable,
            manifest_root=manifest_root,
        )
        build_script_lines = build_script_context.render()

        # Determine noarch setting
        noarch_kind = NoArchKind.python() if backend_config.is_noarch() else None

        # Read pyproject.toml
        pyproject_manifest = read_pyproject_toml(manifest_root)

        # Extract entry points
        entry_points = extract_entry_points(pyproject_manifest)

        # Update recipe components
        recipe.build.python = Python(entry_points=entry_points)
        recipe.build.noarch = noarch_kind
        recipe.build.script = Script(
            content=build_script_lines,
            env=backend_config.env,
        )

        return generated_recipe

    def extract_input_globs_from_build(self, config: PythonBackendConfig, workdir: Path, editable: bool) -> List[str]:
        """Extract input globs for the build."""
        return get_build_input_globs(config, workdir, editable)
