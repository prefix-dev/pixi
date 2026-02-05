from pathlib import Path
import pytest

from pixi_build_ros.metadata_provider import (
    PackageXmlMetadataProvider,
    ROSPackageXmlMetadataProvider,
)


def test_metadata_provider(package_xmls: Path):
    """Test the MetaDataProvider class."""
    package_xml_path = package_xmls / "custom_ros.xml"
    metadata_provider = PackageXmlMetadataProvider(str(package_xml_path), str(package_xmls))
    assert metadata_provider.name() == "custom_ros"
    assert metadata_provider.version() == "0.0.1"
    assert metadata_provider.license() == "LicenseRef-Apache License 2.0"
    assert metadata_provider.description() == "Demo"
    assert metadata_provider.homepage() == "https://test.io/custom_ros"
    assert metadata_provider.repository() == "https://github.com/test/custom_ros"
    assert metadata_provider.license_files() is None


def test_ros_metadata_provider(package_xmls: Path):
    """Test the RosMetaDataProvider class."""
    package_xml_path = package_xmls / "custom_ros.xml"
    metadata_provider = ROSPackageXmlMetadataProvider(str(package_xml_path), str(package_xmls), distro_name="noetic")
    assert metadata_provider.name() == "ros-noetic-custom-ros"
    assert metadata_provider.version() == "0.0.1"
    assert metadata_provider.license() == "LicenseRef-Apache License 2.0"
    assert metadata_provider.description() == "Demo"
    assert metadata_provider.homepage() == "https://test.io/custom_ros"
    assert metadata_provider.repository() == "https://github.com/test/custom_ros"
    assert metadata_provider.license_files() is None


def test_metadata_provider_raises_on_broken_xml(package_xmls: Path):
    """Test that metadata provider raises an error when parsing broken XML."""
    broken_xml_path = package_xmls / "broken.xml"

    with pytest.raises(RuntimeError) as exc_info:
        ROSPackageXmlMetadataProvider(str(broken_xml_path), str(package_xmls), distro_name="noetic")

    # Verify the exception contains location information
    error = exc_info.value
    assert "Failed to parse package.xml" in str(error)


def test_metadata_provider_includes_package_mapping_files_in_input_globs():
    """Test that package mapping files from config are included in input_globs."""
    import tempfile
    import yaml
    from pixi_build_ros.config import ROSBackendConfig

    with tempfile.TemporaryDirectory() as temp_dir:
        temp_path = Path(temp_dir)

        # Create a package.xml inline
        package_xml_content = """<?xml version="1.0"?>
<package format="2">
  <name>test_package</name>
  <version>1.0.0</version>
  <description>Test package</description>
  <maintainer email="test@test.com">Test</maintainer>
  <license>Apache-2.0</license>
</package>
"""
        package_xml_path = temp_path / "package.xml"
        package_xml_path.write_text(package_xml_content)

        # Create a mapping file inline
        mapping_content = {"custom_package": {"conda": ["custom-conda-package"]}}
        mapping_file_path = temp_path / "custom_mapping.yaml"
        with open(mapping_file_path, "w") as f:
            yaml.dump(mapping_content, f)

        # Create config with the mapping file
        config = {
            "distro": "noetic",
            "extra-package-mappings": [str(mapping_file_path)],
        }

        backend_config = ROSBackendConfig.model_validate(config, context={"manifest_root": temp_path})

        # Get package mapping file paths
        package_mapping_files = [str(path) for path in backend_config.get_package_mapping_file_paths()]

        # Create metadata provider
        metadata_provider = ROSPackageXmlMetadataProvider(
            str(package_xml_path),
            str(temp_path),
            distro_name="noetic",
            package_mapping_files=package_mapping_files,
        )

        # Get input globs
        input_globs = metadata_provider.input_globs()

        # Verify the mapping file is included
        assert str(mapping_file_path) in input_globs

        # Verify base globs are still present
        assert "package.xml" in input_globs
        assert "CMakeLists.txt" in input_globs
