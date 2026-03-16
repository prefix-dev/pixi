import pprint

from rosdistro import get_cached_distribution, get_index, get_index_url
from rosdistro.manifest_provider import get_release_tag

from pixi_build_ros.distro import Distro


def test_rosdistro():
    """Testing rosdistro tools to access ROS distributions."""
    index = get_index(get_index_url())
    pprint.pprint(index.distributions)

    distro_name = "jazzy"

    distro = get_cached_distribution(index, distro_name)
    python_version = index.distributions[distro_name]["python_version"]
    distribution_type = index.distributions[distro_name]["distribution_type"]

    print(f"Distribution: {distro.name}")
    print(f"Version: {distro.version}")
    print(f"Python Version: {python_version}")
    print(f"Distribution Type: {distribution_type}")

    pkg_name = "rclcpp"
    pkg = distro.release_packages[pkg_name]
    repo = distro.repositories[pkg.repository_name].release_repository
    print(f"Release Tag: {get_release_tag(repo, pkg_name)}")


def test_distro_class(distro: Distro):
    """Testing the Distro class."""
    distro_name = "jazzy"

    print(f"Distro Name: {distro.name}")
    print(f"Is ROS1: {distro.check_ros1()}")
    print(f"Python Version: {distro.get_python_version()}")

    package_names = distro.get_package_names()
    print(f"Packages in {distro_name}: {package_names}")

    assert distro.has_package("rclcpp"), f"Package rclcpp should exist in {distro_name} distribution."
    assert not distro.has_package("non_existent_package"), (
        f"Package non_existent_package should not exist in {distro_name} distribution."
    )
