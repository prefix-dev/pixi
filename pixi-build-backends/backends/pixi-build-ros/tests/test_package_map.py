from pathlib import Path
import tempfile
import pytest
from pixi_build_backend.types.platform import Platform
from pixi_build_backend.types.project_model import ProjectModel

from pixi_build_ros.distro import Distro
from pixi_build_ros.ros_generator import ROSGenerator, ROSBackendConfig
from pixi_build_ros.utils import load_package_map_data, PackageMappingSource


def test_package_loading(test_data_dir: Path):
    """Load the package map with overwrites."""
    robostack_file = Path(__file__).parent.parent / "robostack.yaml"
    other_package_map = test_data_dir / "other_package_map.yaml"
    result = load_package_map_data(
        [
            PackageMappingSource.from_file(other_package_map),
            PackageMappingSource.from_file(robostack_file),
        ]
    )
    assert "new_package" in result
    assert result["new_package"]["conda"] == "new-package", "Should be added"
    assert result["alsa-oss"]["conda"] == ["other-alsa-oss"], "Should be overwritten"
    assert "robostack" not in result["alsa-oss"], "Should be overwritten due to priority of package maps"
    assert "zlib" in result, "Should still be present"


def test_package_loading_with_inline_mappings(test_data_dir: Path, distro_noetic: Distro):
    """Load package map data from a mix of files and inline entries."""
    robostack_file = Path(__file__).parent.parent / "robostack.yaml"
    inline_entries = {
        "inline-package": {"conda": ["inline-conda"]},
        "inline-ros": {"ros": ["inline-ros"]},
    }
    # Create config for ROS backend
    config = {
        "distro": distro_noetic,
        "noarch": False,
        "extra-package-mappings": [
            "other_package_map.yaml",
            inline_entries,
            robostack_file,
        ],
    }
    parsed_config = ROSBackendConfig.model_validate(config, context={"manifest_root": test_data_dir})
    result = load_package_map_data(parsed_config.extra_package_mappings)

    assert result["inline-package"]["conda"] == ["inline-conda"]
    assert result["inline-ros"]["ros"] == ["inline-ros"]
    assert "zlib" in result, "Should still contain base entries"


def test_generate_recipe_with_custom_ros(package_xmls: Path, test_data_dir: Path, distro_noetic: Distro):
    """Test the generate_recipe function of ROSGenerator."""
    # Create a temporary directory to simulate the package directory
    with tempfile.TemporaryDirectory() as temp_dir:
        temp_path = Path(temp_dir)

        # Copy the test package.xml to the temp directory
        package_xml_source = package_xmls / "custom_ros.xml"
        package_xml_dest = temp_path / "package.xml"
        package_xml_dest.write_text(package_xml_source.read_text(encoding="utf-8"))

        # Create a minimal ProjectModel instance
        model = ProjectModel()

        # Create config for ROS backend
        config = {
            "distro": distro_noetic,
            "noarch": False,
            "extra-package-mappings": [str(test_data_dir / "other_package_map.yaml")],
        }

        # Create host platform
        host_platform = Platform.current()

        # Create ROSGenerator instance
        generator = ROSGenerator()

        # Generate the recipe
        generated_recipe = generator.generate_recipe(
            model=model,
            config=config,
            manifest_path=str(temp_path),
            host_platform=host_platform,
        )

        # Verify the generated recipe has the expected requirements
        assert generated_recipe.recipe.package.name.get_concrete() == "ros-noetic-custom-ros"

        req_string = list(str(req) for req in generated_recipe.recipe.requirements.run)
        assert "ros-noetic-ros-package" in req_string
        assert "ros-noetic-ros-package-msgs" in req_string
        assert "multi-package-a" in req_string
        assert "multi-package-b" in req_string


def test_generate_recipe_with_inline_package_mappings(package_xmls: Path, test_data_dir: Path, distro_noetic: Distro):
    """Inline entries should be merged into the package map."""

    with tempfile.TemporaryDirectory() as temp_dir:
        temp_path = Path(temp_dir)

        package_xml_source = package_xmls / "custom_ros.xml"
        package_xml_dest = temp_path / "package.xml"
        package_xml_dest.write_text(package_xml_source.read_text(encoding="utf-8"))

        model = ProjectModel()

        config = {
            "distro": distro_noetic,
            "noarch": False,
            "extra-package-mappings": [
                {"ros_package": {"ros": ["ros-custom2", "ros-custom2-msgs"]}},
                str(test_data_dir / "other_package_map.yaml"),
            ],
        }

        host_platform = Platform.current()

        generator = ROSGenerator()

        generated_recipe = generator.generate_recipe(
            model=model,
            config=config,
            manifest_path=str(temp_path),
            host_platform=host_platform,
        )

        req_string = list(str(req) for req in generated_recipe.recipe.requirements.run)
        assert "ros-noetic-ros-custom2" in req_string
        assert "ros-noetic-ros-custom2-msgs" in req_string


def test_package_map_does_not_exist(package_xmls: Path, test_data_dir: Path, distro_noetic: Distro):
    """Test the generate_recipe function of ROSGenerator."""
    # Create a temporary directory to simulate the package directory
    with tempfile.TemporaryDirectory() as temp_dir:
        temp_path = Path(temp_dir)

        # Copy the test package.xml to the temp directory
        package_xml_source = package_xmls / "custom_ros.xml"
        package_xml_dest = temp_path / "package.xml"
        package_xml_dest.write_text(package_xml_source.read_text(encoding="utf-8"))

        # Create a minimal ProjectModel instance
        model = ProjectModel()

        # Create config for ROS backend
        config = {
            "distro": distro_noetic,
            "noarch": False,
            "extra-package-mappings": [("does-not-exist.yaml")],
        }

        # Create host platform
        host_platform = Platform.current()

        # Create ROSGenerator instance
        generator = ROSGenerator()

        with pytest.raises(ValueError) as excinfo:
            generator.generate_recipe(
                model=model,
                config=config,
                manifest_path=str(temp_path),
                host_platform=host_platform,
            )
        assert "Additional package map file" in str(excinfo.value)
