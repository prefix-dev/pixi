import dataclasses
import os
from itertools import chain
from pathlib import Path
from typing import Any

from catkin_pkg.package import Package as CatkinPackage, parse_package_string, Dependency

from pixi_build_backend.types.intermediate_recipe import ConditionalRequirements
from pixi_build_backend.types.item import ItemPackageDependency, VecItemPackageDependency
from pixi_build_backend.types.platform import Platform
from pixi_build_ros.distro import Distro
from rattler import Version
from .config import PackageMapEntry, PackageMappingSource


@dataclasses.dataclass
class PackageNameWithSpec:
    """Package name with spec."""

    name: str
    spec: str | None = None


# Any in here means ROSBackendConfig
def get_build_input_globs(config: dict[str, Any], editable: bool) -> list[str]:
    """Get build input globs for ROS package."""
    base_globs = [
        # Source files
        "**/*.c",
        "**/*.cpp",
        "**/*.h",
        "**/*.hpp",
        "**/*.rs",
        "**/*.sh",
        # Project configuration
        "package.xml",
        "setup.py",
        "setup.cfg",
        "pyproject.toml",
        # Build configuration
        "Makefile",
        "CMakeLists.txt",
        "MANIFEST.in",
        "tests/**/*.py",
        "docs/**/*.rst",
        "docs/**/*.md",
    ]

    python_globs = [] if editable else ["**/*.py", "**/*.pyx"]

    all_globs = base_globs + python_globs
    if config.get("extra-input-globs"):
        all_globs.extend(config["extra-input-globs"])
    return all_globs


def get_package_xml_content(package_xml_path: Path) -> str:
    """Read package.xml file from the manifest root."""
    if not package_xml_path.exists():
        raise FileNotFoundError(f"package.xml not found at {package_xml_path}")

    with open(package_xml_path) as f:
        return f.read()


def convert_package_xml_to_catkin_package(package_xml_content: str) -> CatkinPackage:
    """Convert package.xml content to a CatkinPackage object."""
    package_reading_warnings = None
    package_xml = parse_package_string(package_xml_content, package_reading_warnings)

    # Evaluate conditions in the package.xml
    # TODO: validate the need for dealing with configuration conditions
    package_xml.evaluate_conditions(os.environ)

    return package_xml


def load_package_map_data(package_map_sources: list[PackageMappingSource]) -> dict[str, PackageMapEntry]:
    """Load and merge package map data from files and inline mappings."""

    result: dict[str, PackageMapEntry] = {}
    for source in reversed(package_map_sources):
        result.update(source.get_package_mapping())
    return result


def rosdep_to_conda_package_spec(
    dep: Dependency,
    distro: Distro,
    host_platform: Platform,
    package_map_data: dict[str, PackageMapEntry],
) -> list[str]:
    """Convert a ROS dependency name to a conda package spec."""
    if host_platform.is_linux:
        target_platform = "linux"
    elif host_platform.is_windows:
        target_platform = "win64"
    elif host_platform.is_osx:
        target_platform = "osx"
    else:
        raise RuntimeError(f"Unsupported platform: {host_platform}")

    spec_str = rosdep_nameless_matchspec(dep)

    if dep.name not in package_map_data:
        # Package name isn't found in the package map, so we are going to assume it is a ROS package.
        return [f"ros-{distro.name}-{dep.name.replace('_', '-')}{spec_str or ''}"]

    # Dependency found in package map

    # Case 1: It's a custom ROS dependency
    if "ros" in package_map_data[dep.name]:
        return [f"ros-{distro.name}-{dep.replace('_', '-')}{spec_str}" for dep in package_map_data[dep.name]["ros"]]

    # Case 2: It's a custom package name
    elif "conda" in package_map_data[dep.name] or "robostack" in package_map_data[dep.name]:
        # determine key
        key = "robostack" if "robostack" in package_map_data[dep.name] else "conda"

        # Get the conda packages for the dependency
        conda_packages = package_map_data[dep.name].get(key, [])

        if isinstance(conda_packages, dict):
            # TODO: Handle different platforms
            conda_packages = conda_packages.get(target_platform, [])

        additional_packages = []
        # Deduplicate of the code in:
        # https://github.com/RoboStack/vinca/blob/7d3a05e01d6898201a66ba2cf6ea771250671f58/vinca/main.py#L562
        if "REQUIRE_GL" in conda_packages:
            conda_packages.remove("REQUIRE_GL")
            if "linux" in target_platform:
                additional_packages.append("libgl-devel")
        if "REQUIRE_OPENGL" in conda_packages:
            conda_packages.remove("REQUIRE_OPENGL")
            if "linux" in target_platform:
                # TODO: this should only go into the host dependencies
                additional_packages.extend(["libgl-devel", "libopengl-devel"])
            if target_platform in ["linux", "osx", "unix"]:
                # TODO: force this into the run dependencies
                additional_packages.extend(["xorg-libx11", "xorg-libxext"])

        # Add the version specifier if it exists and it is only one package defined
        if spec_str:
            if len(conda_packages) == 1:
                if " " not in conda_packages[0]:
                    conda_packages = [f"{conda_packages[0]}{spec_str}"]
                else:
                    raise ValueError(
                        f"Version specifier can only be used for a package without constraint already present, "
                        f"but found {conda_packages[0]} for {dep.name} "
                        f"in the package map."
                    )
            else:
                raise ValueError(
                    f"Version specifier can only be used for one package, "
                    f"but found {len(conda_packages)} packages for {dep.name} "
                    f"in the package map."
                )

        return conda_packages + additional_packages
    else:
        raise ValueError(f"Unknown package map entry: {dep.name}.")


def rosdep_nameless_matchspec(dep: Dependency) -> str:
    """Format the version constraints from a ros package.xml to a nameless matchspec"""
    right_ineq = [dep.version_lt, dep.version_lte]
    left_ineq = [dep.version_gt, dep.version_gte]
    eq = dep.version_eq

    for version in left_ineq + right_ineq + [eq]:
        if version is None:
            continue
        elif version == "":
            raise ValueError(
                f"Incorrect version specification in package.xml: '{dep.name}': version is empty string (\"\")"
            )
        try:
            # check if we can parse the version
            Version(version)
        except TypeError as e:
            raise ValueError(
                f"Incorrect version specification in package.xml: '{dep.name}' at version '{version}' "
            ) from e

    def not_none(p: Any) -> bool:
        return p is not None

    if all(map(not_none, right_ineq)):
        raise ValueError(f"Dependency {dep.name} cannot be specified by both `<` and `<=`")
    if all(map(not_none, left_ineq)):
        raise ValueError(f"Dependency {dep.name} cannot be specified by both `>` and `>=`")

    some_inequality = any(map(lambda p: p is not None, right_ineq + left_ineq))
    if eq and some_inequality:
        raise ValueError(f"Dependency {dep.name} cannot be specified by both `=` and some inequality")

    if eq:
        return f"=={eq}"

    pair = []

    if dep.version_gt:
        pair.append(f">{dep.version_gt}")
    if dep.version_gte:
        pair.append(f">={dep.version_gte}")

    if dep.version_lt:
        pair.append(f"<{dep.version_lt}")
    if dep.version_lte:
        pair.append(f"<={dep.version_lte}")

    res = ",".join(pair)

    return " " + res if len(pair) > 0 else res


def package_xml_to_conda_requirements(
    pkg: CatkinPackage,
    distro: Distro,
    host_platform: Platform,
    package_map_data: dict[str, PackageMapEntry],
) -> ConditionalRequirements:
    """Convert a CatkinPackage to ConditionalRequirements for conda."""
    # All build related dependencies go into the build requirements
    build_deps = pkg.buildtool_depends
    # TODO: should the export dependencies be included here?
    build_deps += pkg.buildtool_export_depends
    build_deps += pkg.build_depends
    build_deps += pkg.build_export_depends
    # Also add test dependencies, because they might be needed during build (i.e. for pytest/catch2 etc in CMake macros)
    build_deps += pkg.test_depends
    build_deps = [d for d in build_deps if d.evaluated_condition]
    # Add the ros_workspace dependency as a default build dependency for ros2 packages
    if not distro.check_ros1():
        build_deps += [Dependency(name="ros_workspace")]
    conda_build_deps_chain = [
        rosdep_to_conda_package_spec(dep, distro, host_platform, package_map_data) for dep in build_deps
    ]
    conda_build_deps = list(chain.from_iterable(conda_build_deps_chain))

    run_deps = pkg.run_depends
    run_deps += pkg.exec_depends
    run_deps += pkg.build_export_depends
    run_deps += pkg.buildtool_export_depends
    run_deps = [d for d in run_deps if d.evaluated_condition]
    conda_run_deps_chain = [
        rosdep_to_conda_package_spec(dep, distro, host_platform, package_map_data) for dep in run_deps
    ]
    conda_run_deps = list(chain.from_iterable(conda_run_deps_chain))

    build_requirements = [ItemPackageDependency(name) for name in conda_build_deps]
    run_requirements = [ItemPackageDependency(name) for name in conda_run_deps]

    cond = ConditionalRequirements()
    # TODO: should we add all build dependencies to the host requirements?
    cond.host = build_requirements
    cond.build = build_requirements
    cond.run = run_requirements

    return cond


def find_matching(list_to_find: list[ItemPackageDependency], name: str) -> ItemPackageDependency | None:
    for dep in list_to_find:
        if dep.concrete.package_name == name:
            return dep
    else:
        return None


def normalize_spec(spec: str | None, package_name: str) -> str:
    """Normalize a spec by removing package name and handling None."""
    if not spec:
        return ""
    return spec.removeprefix(package_name).strip()


def merge_specs(spec1: str | None, spec2: str | None, package_name: str) -> str:
    # remove the package name
    version_spec1 = normalize_spec(spec1, package_name)
    version_spec2 = normalize_spec(spec2, package_name)

    if " " in version_spec1 or " " in version_spec2:
        raise ValueError(f"{version_spec1}, or {version_spec2} contains spaces, cannot merge specifiers.")

    # early out with *, empty or ==
    if version_spec1 in ["*", ""] or "==" in version_spec2 or version_spec1 == version_spec2:
        return spec2 or ""
    if version_spec2 in ["*", ""] or "==" in version_spec1:
        return spec1 or ""
    return package_name + " " + ",".join([version_spec1, version_spec2])


def merge_unique_items(
    model: list[ItemPackageDependency] | VecItemPackageDependency,
    package: list[ItemPackageDependency] | VecItemPackageDependency,
) -> list[ItemPackageDependency]:
    """Merge unique items from source into target."""

    result: list[ItemPackageDependency] = []
    templates_in_model = [str(i.template) for i in model]
    for item in list(model) + list(package):
        # It's concrete (i.e. no template)
        if item.concrete is not None:
            # It does not exist yet in model
            item_in_result = find_matching(result, item.concrete.package_name)
            if not item_in_result:
                result.append(item)
            elif item_in_result.concrete.is_source:
                # If the existing dependency is source, don't merge - keep the source one
                continue
            elif item.concrete.is_source:
                # If a new item is source, replace the existing one
                result = [dep for dep in result if dep.concrete.package_name != item.concrete.package_name]
                result.append(item)
            else:
                new_dep = ItemPackageDependency(
                    name=merge_specs(
                        item_in_result.concrete.binary_spec, item.concrete.binary_spec, item.concrete.package_name
                    )
                )
                result.remove(item_in_result)
                result.append(new_dep)

        elif str(item.template) not in templates_in_model:
            result.append(item)
    return result


def merge_requirements(
    model_requirements: ConditionalRequirements,
    package_requirements: ConditionalRequirements,
) -> ConditionalRequirements:
    """Merge two sets of requirements."""
    merged = ConditionalRequirements()

    merged.host = merge_unique_items(model_requirements.host, package_requirements.host)
    merged.build = merge_unique_items(model_requirements.build, package_requirements.build)
    merged.run = merge_unique_items(model_requirements.run, package_requirements.run)

    # If the dependency is of type Source in one of the requirements, we need to set them to Source for all variants
    return merged
