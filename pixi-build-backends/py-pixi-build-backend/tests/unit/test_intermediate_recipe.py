from pathlib import Path
from typing import Any
from pixi_build_backend.types.intermediate_recipe import IntermediateRecipe, Python
from pixi_build_backend.types.item import ItemPackageDependency


def test_from_yaml(snapshot: Any) -> None:
    yaml_file = Path(__file__).parent.parent / "data" / "boltons_recipe.yaml"
    yaml_content = yaml_file.read_text()

    recipe = IntermediateRecipe.from_yaml(yaml_content)

    assert snapshot == recipe.to_yaml()


def test_nested_setters() -> None:
    yaml_file = Path(__file__).parent.parent / "data" / "boltons_recipe.yaml"
    yaml_content = yaml_file.read_text()

    recipe = IntermediateRecipe.from_yaml(yaml_content)
    # Test that we can access the package name
    original_name = str(recipe.package.name)
    assert isinstance(original_name, str)
    assert len(original_name) > 0


def test_intermediate_str(snapshot: Any) -> None:
    yaml_file = Path(__file__).parent.parent / "data" / "boltons_recipe.yaml"
    yaml_content = yaml_file.read_text()

    recipe = IntermediateRecipe.from_yaml(yaml_content)

    assert str(recipe) == snapshot


def test_we_can_create_python() -> None:
    py = Python(["entry-point=module:function"])

    assert py.entry_points == ["entry-point = module:function"]


def test_package_types() -> None:
    package = ItemPackageDependency("test")
    assert package.concrete is not None
    assert str(package.concrete.package_name) == "test"


def test_package_types_conditional() -> None:
    package = ItemPackageDependency("test")
    assert package.concrete is not None
    assert str(package.concrete.package_name) == "test"

    package = ItemPackageDependency("${{ compiler('c') }}")

    assert package.concrete is None
    assert package.template
