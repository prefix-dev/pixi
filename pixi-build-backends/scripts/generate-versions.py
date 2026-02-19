"""Generate version environment variables for pixi-build-backends CI."""

import json
import subprocess
import tomllib
from datetime import datetime
from pathlib import Path


def get_git_short_hash() -> str:
    """Get the short git hash of the current HEAD."""
    result = subprocess.run(
        ["git", "rev-parse", "--short=7", "HEAD"],
        capture_output=True,
        text=True,
        check=True,
    )
    return result.stdout.strip()


def main():
    # Find the repo root
    script_dir = Path(__file__).parent
    repo_root = script_dir.parent.parent

    # Generate version suffix
    now = datetime.now()
    date_suffix = now.strftime("%Y%m%d")
    time_suffix = now.strftime("%H%M")
    git_hash = get_git_short_hash()
    version_suffix = f"{date_suffix}.{time_suffix}.{git_hash}"

    # Get Rust package versions from cargo metadata
    result = subprocess.run(
        ["cargo", "metadata", "--format-version=1", "--no-deps"],
        capture_output=True,
        text=True,
        check=True,
        cwd=repo_root,
    )
    cargo_metadata = json.loads(result.stdout)

    env_vars = {}

    # Rust backends
    rust_packages = [
        "pixi_build_cmake",
        "pixi_build_mojo",
        "pixi_build_python",
        "pixi_build_rattler_build",
        "pixi_build_rust",
    ]
    for package in cargo_metadata.get("packages", []):
        if package["name"] in rust_packages:
            env_name = package["name"].replace("-", "_").upper() + "_VERSION"
            env_vars[env_name] = f"{package['version']}.{version_suffix}"

    # py-pixi-build-backend (separate workspace)
    py_backend_cargo = repo_root / "pixi-build-backends" / "py-pixi-build-backend" / "Cargo.toml"
    with open(py_backend_cargo, "rb") as f:
        cargo_toml = tomllib.load(f)
        version = cargo_toml["package"]["version"]
        env_vars["PY_PIXI_BUILD_BACKEND_VERSION"] = f"{version}.{version_suffix}"

    # pixi-build-ros (Python package)
    ros_pyproject = (
        repo_root / "pixi-build-backends" / "backends" / "pixi-build-ros" / "pyproject.toml"
    )
    with open(ros_pyproject, "rb") as f:
        pyproject = tomllib.load(f)
        version = pyproject["project"]["version"]
        env_vars["PIXI_BUILD_ROS_VERSION"] = f"{version}.{version_suffix}"

    for name, value in env_vars.items():
        print(f"{name}={value}")


if __name__ == "__main__":
    main()
