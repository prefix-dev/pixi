from typing import Any

from pixi_build_backend.types.conditional import ConditionalPackageDependency, ListOrItemPackageDependency
from pixi_build_backend.types.generated_recipe import GeneratedRecipe
from pixi_build_backend.types.item import ItemPackageDependency
from pixi_build_backend.types.project_model import ProjectModel
from pixi_build_backend.types.intermediate_recipe import IntermediateRecipe


def test_generated_recipe_from_model(snapshot: Any) -> None:
    """Test initialization of ProjectModel."""
    model = ProjectModel(name="test_project", version="1.0.0")

    generated_recipe = GeneratedRecipe.from_model(model)

    # Verify that the recipe is of the correct type
    assert isinstance(generated_recipe.recipe, IntermediateRecipe)

    assert snapshot == generated_recipe.recipe.to_yaml()


def test_setting_package_name_from_generated_recipe() -> None:
    """Test initialization of ProjectModel."""
    model = ProjectModel(name="test_project", version="1.0.0")

    generated_recipe = GeneratedRecipe.from_model(model)

    # Test that we can access the package name
    original_name = str(generated_recipe.recipe.package.name)
    assert isinstance(original_name, str)
    assert len(original_name) > 0


def test_package_dependency_modification() -> None:
    """Test initialization of ProjectModel."""
    model = ProjectModel(name="test_project", version="1.0.0")

    generated_recipe = GeneratedRecipe.from_model(model)

    generated_recipe.recipe.requirements.build.append(ItemPackageDependency("test_package"))
    generated_recipe.recipe.requirements.build.append(ItemPackageDependency("test_package"))

    assert len(generated_recipe.recipe.requirements.build) == 2


def test_conditional_item() -> None:
    """Test setting of conditional item."""

    conditional = ConditionalPackageDependency(
        "os == 'linux'", ListOrItemPackageDependency(["package1"]), ListOrItemPackageDependency(["package3"])
    )
    item = ItemPackageDependency.new_from_conditional(conditional)

    if item.conditional is not None:
        item.conditional.condition = "foo-bar"

    # this is a known issue with the current implementation
    if item.conditional is not None:
        assert item.conditional.condition == "os == 'linux'"


def test_generated_recipe_setting_version() -> None:
    """Test initialization of ProjectModel."""
    model = ProjectModel(name="test_project", version="1.0.0")

    generated_recipe = GeneratedRecipe.from_model(model)

    # Test that the version can be accessed as a string
    version_str = str(generated_recipe.recipe.package.version)
    assert isinstance(version_str, str)
    assert len(version_str) > 0

    # Test getting concrete version
    concrete = generated_recipe.recipe.package.version.get_concrete()
    assert concrete is not None

    # Verify concrete version is a valid string
    assert isinstance(concrete, str)
    assert len(concrete) > 0
