from pathlib import Path
from typing import Any, Dict, Optional, Protocol, List
from pixi_build_backend.types.intermediate_recipe import IntermediateRecipe
from pixi_build_backend.pixi_build_backend import PyGeneratedRecipe, PyGenerateRecipe
from pixi_build_backend.types.platform import Platform
from pixi_build_backend.types.project_model import ProjectModel
from pixi_build_backend.types.python_params import PythonParams
from pixi_build_backend.types.metadata_provider import MetadataProvider


class GeneratedRecipe:
    """A generated recipe wrapper."""

    _inner: PyGeneratedRecipe

    def __init__(self) -> None:
        self._inner = PyGeneratedRecipe()

    @classmethod
    def from_model(cls, model: ProjectModel, metadata_provider: Optional[MetadataProvider] = None) -> "GeneratedRecipe":
        """Create a GeneratedRecipe from a ProjectModel."""
        instance = cls()
        if metadata_provider is not None:
            instance._inner = PyGeneratedRecipe().from_model_with_provider(model._inner, metadata_provider)
        else:
            instance._inner = PyGeneratedRecipe().from_model(model._inner)
        return instance

    @property
    def recipe(self) -> IntermediateRecipe:
        """Get the recipe."""
        return IntermediateRecipe._from_inner(self._inner.recipe)

    @recipe.setter
    def recipe(self, value: IntermediateRecipe) -> None:
        """Set the recipe."""
        self._inner.recipe = value._inner

    @property
    def metadata_input_globs(self) -> List[str]:
        """Get the metadata input globs."""
        return self._inner.metadata_input_globs

    @property
    def build_input_globs(self) -> List[str]:
        """Get the build input globs."""
        return self._inner.build_input_globs

    def __repr__(self) -> str:
        return self._inner.__repr__()

    def _into_py(self) -> PyGeneratedRecipe:
        """
        Converts this object into a type that can be used by the Rust code.
        """
        return self._inner


class GenerateRecipeProtocol(Protocol):
    """
    Protocol for generating recipes.
    This should be implemented by the Python generator.
    """

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
        ...

    def extract_input_globs_from_build(self, config: dict[str, Any], workdir: Path, editable: bool) -> List[str]:
        """Extract input globs for the build."""
        ...

    def default_variants(self, host_platform: Platform) -> Dict[str, Any]:
        """Get the default variants for the generator."""
        ...


class GenerateRecipe:
    """Protocol for generating recipes."""

    _inner: PyGenerateRecipe

    def __init__(self, instance: GenerateRecipeProtocol):
        self._inner = PyGenerateRecipe(instance)

    def _into_py(self) -> PyGenerateRecipe:
        """
        Converts this object into a type that can be used by the Rust code.
        """
        return self._inner
