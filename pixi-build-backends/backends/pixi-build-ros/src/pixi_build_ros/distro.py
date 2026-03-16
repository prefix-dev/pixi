from typing import cast

from rosdistro import get_cached_distribution, get_index, get_index_url

# This modifies the file


class Distro:
    def __init__(self, distro_name: str):
        index = get_index(get_index_url())
        self._distro = get_cached_distribution(index, distro_name)
        self.distro_name = distro_name

        # cache distribution type
        self._distribution_type: str = index.distributions[distro_name]["distribution_type"]
        self._python_version: str = index.distributions[distro_name]["python_version"]

    @property
    def name(self) -> str:
        return self.distro_name

    def check_ros1(self) -> bool:
        return self._distribution_type == "ros1"

    @property
    def ros_distro_mutex_name(self) -> str:
        return f"ros{'' if self.check_ros1() else '2'}-distro-mutex"

    def get_python_version(self) -> str:
        return self._python_version

    def get_package_names(self) -> list[str]:
        return cast(list[str], self._distro.release_packages.keys())

    def has_package(self, package_name: str) -> bool:
        """Check if the distribution has a specific package."""
        return package_name in self._distro.release_packages
