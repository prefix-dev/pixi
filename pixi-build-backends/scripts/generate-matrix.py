import json
import subprocess
import sys
import tomllib
from datetime import datetime
from pathlib import Path


def get_git_short_hash():
    """Get the short git hash of the current HEAD"""
    result = subprocess.run(
        ["git", "rev-parse", "--short=7", "HEAD"],
        capture_output=True,
        text=True,
        check=True,
    )
    return result.stdout.strip()


def get_current_date():
    """Get current date in yyyymmdd format"""
    return datetime.now().strftime("%Y%m%d")


def get_current_time():
    """Get current time in HHMM format"""
    return datetime.now().strftime("%H%M")


def generate_matrix(filter_package_name=None):
    # Find the repo root (where Cargo.toml is)
    script_dir = Path(__file__).parent
    repo_root = script_dir.parent.parent  # pixi-build-backends/scripts -> pixi/

    # Run cargo metadata from the repo root
    result = subprocess.run(
        ["cargo", "metadata", "--format-version=1", "--no-deps"],
        capture_output=True,
        text=True,
        check=True,
        cwd=repo_root,
    )
    cargo_metadata = json.loads(result.stdout)

    # Get all packages with binary targets that are pixi-build backends
    all_packages = []

    if "packages" in cargo_metadata:
        for package in cargo_metadata["packages"]:
            # Only include pixi-build-* packages with binary targets
            if not package["name"].startswith("pixi-build-"):
                continue

            has_binary = any(target["kind"][0] == "bin" for target in package.get("targets", []))

            if has_binary:
                all_packages.append(
                    {"name": package["name"], "version": package["version"], "type": "rust"}
                )

    # Add py-pixi-build-backend manually since it's in a separate directory
    py_backend_cargo = repo_root / "pixi-build-backends" / "py-pixi-build-backend" / "Cargo.toml"
    if py_backend_cargo.exists():
        with open(py_backend_cargo, "rb") as f:
            cargo_toml = tomllib.load(f)
            all_packages.append(
                {
                    "name": "py-pixi-build-backend",
                    "version": cargo_toml["package"]["version"],
                    "type": "python",
                }
            )

    # Add pixi-build-ros manually since it's a Python package in backends/
    ros_pyproject = (
        repo_root / "pixi-build-backends" / "backends" / "pixi-build-ros" / "pyproject.toml"
    )
    if ros_pyproject.exists():
        with open(ros_pyproject, "rb") as f:
            pyproject_toml = tomllib.load(f)
            all_packages.append(
                {
                    "name": "pixi-build-ros",
                    "version": pyproject_toml["project"]["version"],
                    "type": "python",
                }
            )

    # Filter packages by name if specified
    if filter_package_name:
        available_packages = [pkg["name"] for pkg in all_packages]
        filtered_packages = [pkg for pkg in all_packages if pkg["name"] == filter_package_name]
        if not filtered_packages:
            raise ValueError(
                f"Package '{filter_package_name}' not found. Available packages: {', '.join(available_packages)}"
            )
        all_packages = filtered_packages
        print(f"Filtering to package: {filter_package_name}", file=sys.stderr)

    # this is to overcome the issue of matrix generation from github actions side
    # https://github.com/orgs/community/discussions/67591
    targets = [
        {"target": "linux-64", "os": "ubuntu-latest"},
        {"target": "linux-aarch64", "os": "ubuntu-latest"},
        {"target": "linux-ppc64le", "os": "ubuntu-latest"},
        {"target": "win-64", "os": "windows-latest"},
        {"target": "osx-64", "os": "macos-15-intel"},
        {"target": "osx-arm64", "os": "macos-15"},
    ]

    def get_targets_for_package(package_name, all_targets):
        """Get the appropriate targets for a package. Noarch packages only build on linux-64."""
        if package_name == "pixi-build-ros":
            return [t for t in all_targets if t["target"] == "linux-64"]
        else:
            return all_targets

    # Extract bin names, versions, and generate env and recipe names
    matrix = []

    if not all_packages:
        raise ValueError("No packages found")

    # Generate auto-versioning with timestamp
    date_suffix = get_current_date()
    time_suffix = get_current_time()
    git_hash = get_git_short_hash()

    print(
        f"Building all packages with date: {date_suffix}, time: {time_suffix}, git hash: {git_hash}",
        file=sys.stderr,
    )

    package_names = []
    for package in all_packages:
        package_names.append(package["name"])
        # Create auto-version: original_version.yyyymmdd.hhmm.git_hash
        auto_version = f"{package['version']}.{date_suffix}.{time_suffix}.{git_hash}"

        # Generate environment variable name
        if package["name"] == "py-pixi-build-backend":
            env_name = "PY_PIXI_BUILD_BACKEND_VERSION"
        elif package["name"] == "pixi-build-ros":
            env_name = "PIXI_BUILD_ROS_VERSION"
        else:
            env_name = f"{package['name'].replace('-', '_').upper()}_VERSION"

        for target in get_targets_for_package(package["name"], targets):
            matrix.append(
                {
                    "bin": package["name"],
                    "target": target["target"],
                    "version": auto_version,
                    "env_name": env_name,
                    "os": target["os"],
                }
            )

    print(
        f"Found {len(package_names)} packages: {', '.join(package_names)}",
        file=sys.stderr,
    )

    if not matrix:
        raise RuntimeError("No packages found to build")

    matrix_json = json.dumps(matrix)

    # Debug output to stderr so it doesn't interfere with matrix JSON
    print(f"Generated matrix with {len(matrix)} entries", file=sys.stderr)

    print(matrix_json)


if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(description="Generate build matrix for packages")
    parser.add_argument("--package", help="Filter to specific package name")
    args = parser.parse_args()

    generate_matrix(args.package)
