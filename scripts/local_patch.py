"""
Script to convert dependencies in Cargo.toml to use local path dependencies.
Supports UV and Rattler libraries with clean switching that preserves git state.
"""

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path
from typing import Any, Optional


# Library configurations
LIBRARY_CONFIGS: dict[str, dict[str, Any]] = {
    "uv": {
        "pattern": r"^uv-.*$",
        "default_path": "../uv",
        "git_url": "https://github.com/astral-sh/uv",
        "patch_url": "https://github.com/astral-sh/uv",
    },
    "rattler": {
        "pattern": r"^(rattler.*|file_url|simple_spawn_blocking|tools|path_resolver|coalesced_map)$",
        "default_path": "../rattler",
        "git_url": None,  # Rattler deps don't use git currently
        "patch_url": "crates-io",  # Use [patch.crates-io] for rattler
    },
}


def get_local_versions(local_path: str) -> dict[str, str]:
    """Get versions of workspace packages from the local source directory."""
    try:
        # Run cargo metadata in the local directory
        result = subprocess.run(
            ["cargo", "metadata", "--no-deps", "--format-version=1"],
            cwd=local_path,
            capture_output=True,
            text=True,
            check=True,
        )

        metadata = json.loads(result.stdout)
        versions = {}

        # Extract name and version for workspace members
        workspace_member_ids = set(metadata.get("workspace_members", []))
        for package in metadata.get("packages", []):
            if package["id"] in workspace_member_ids:
                versions[package["name"]] = package["version"]

        return versions
    except subprocess.CalledProcessError as e:
        print(f"Error running cargo metadata in {local_path}: {e}")
        return {}
    except json.JSONDecodeError as e:
        print(f"Error parsing cargo metadata JSON: {e}")
        return {}
    except Exception as e:
        print(f"Unexpected error getting versions from {local_path}: {e}")
        return {}


def find_dependencies(cargo_toml_content: str, library: str) -> set[str]:
    """Find all dependencies matching the library pattern in the Cargo.toml content."""
    config = LIBRARY_CONFIGS[library]
    pattern = config["pattern"]
    deps: set[str | Any] = set()

    # Look for dependencies in workspace.dependencies and dependencies sections
    for line in cargo_toml_content.split("\n"):
        # Match dependency lines (with version, git, or path specifications)
        dep_match = re.match(r"^([a-zA-Z0-9_-]+)\s*=", line.strip())
        if dep_match:
            dep_name = dep_match.group(1)
            if re.match(pattern, dep_name):
                deps.add(dep_name)

    return deps


def get_current_workspace_dependency_versions(
    cargo_toml_content: str, library: str
) -> dict[str, str]:
    """Extract current version numbers from [workspace.dependencies] for matching dependencies."""
    config = LIBRARY_CONFIGS[library]
    pattern = config["pattern"]
    lines = cargo_toml_content.split("\n")
    versions = {}
    in_workspace_deps = False

    for line in lines:
        # Track if we're in [workspace.dependencies] section
        if line.strip() == "[workspace.dependencies]":
            in_workspace_deps = True
            continue
        elif line.strip().startswith("[") and line.strip() != "[workspace.dependencies]":
            in_workspace_deps = False
            continue

        # If in workspace.dependencies, check if this is a matching dependency
        if in_workspace_deps:
            dep_match = re.match(r"^([a-zA-Z0-9_-]+)\s*=\s*(.*)$", line.strip())
            if dep_match:
                dep_name = dep_match.group(1)
                dep_value = dep_match.group(2)

                # Check if this dependency matches the pattern
                if re.match(pattern, dep_name):
                    # Extract version from the dependency specification
                    if dep_value.startswith('"') or dep_value.startswith("'"):
                        # Simple string version like: foo = "1.0.0"
                        version_match = re.match(r'["\']([^"\']+)["\']', dep_value)
                        if version_match:
                            versions[dep_name] = version_match.group(1)
                    elif dep_value.startswith("{"):
                        # Inline table like: foo = { version = "1.0.0", ... }
                        version_match = re.search(r'version\s*=\s*"([^"]*)"', dep_value)
                        if version_match:
                            versions[dep_name] = version_match.group(1)

    return versions


def update_workspace_dependency_versions(
    cargo_toml_content: str, library: str, versions: dict[str, str]
) -> str:
    """Update version numbers in [workspace.dependencies] for matching dependencies."""
    config = LIBRARY_CONFIGS[library]
    pattern = config["pattern"]
    lines = cargo_toml_content.split("\n")
    result_lines = []
    in_workspace_deps = False

    for line in lines:
        # Track if we're in [workspace.dependencies] section
        if line.strip() == "[workspace.dependencies]":
            in_workspace_deps = True
            result_lines.append(line)
            continue
        elif line.strip().startswith("[") and line.strip() != "[workspace.dependencies]":
            in_workspace_deps = False
            result_lines.append(line)
            continue

        # If in workspace.dependencies, check if this is a matching dependency
        if in_workspace_deps:
            dep_match = re.match(r"^([a-zA-Z0-9_-]+)\s*=\s*(.*)$", line.strip())
            if dep_match:
                dep_name = dep_match.group(1)
                dep_value = dep_match.group(2)

                # Check if this dependency matches the pattern and we have a version for it
                if re.match(pattern, dep_name) and dep_name in versions:
                    new_version = versions[dep_name]
                    # Try to update the version in the dependency specification
                    # Handle both simple string versions and complex table/inline table syntax
                    if dep_value.startswith('"') or dep_value.startswith("'"):
                        # Simple string version like: foo = "1.0.0"
                        updated_line = f'{dep_name} = "{new_version}"'
                    elif dep_value.startswith("{"):
                        # Inline table like: foo = { version = "1.0.0", ... }
                        updated_value = re.sub(
                            r'version\s*=\s*"[^"]*"',
                            f'version = "{new_version}"',
                            dep_value,
                        )
                        updated_line = f"{dep_name} = {updated_value}"
                    else:
                        # Leave it unchanged if format is not recognized
                        result_lines.append(line)
                        continue

                    # Preserve indentation
                    indent = len(line) - len(line.lstrip())
                    result_lines.append(" " * indent + updated_line)
                    continue

        result_lines.append(line)

    return "\n".join(result_lines)


def has_patch_section(cargo_toml_content: str, library: str) -> bool:
    """Check if there's already a patch section for the library."""
    marker_start = f"# pixi-{library}-patches - START"
    return marker_start in cargo_toml_content


def extract_original_versions_from_patch(
    cargo_toml_content: str, library: str
) -> Optional[dict[str, str]]:
    """Extract original versions from the patch section comment."""
    lines = cargo_toml_content.split("\n")
    marker_start = f"# pixi-{library}-patches - START"
    version_marker = f"# pixi-{library}-original-versions:"

    in_patch_block = False
    for line in lines:
        if line.strip() == marker_start:
            in_patch_block = True
            continue

        if in_patch_block and line.strip().startswith(version_marker):
            # Extract the JSON from the comment
            json_str = line.strip()[len(version_marker) :].strip()
            try:
                return json.loads(json_str)
            except json.JSONDecodeError as e:
                print(f"Warning: Could not parse original versions from comment: {e}")
                return None

        # Stop looking if we hit the end marker
        if in_patch_block and line.strip() == f"# pixi-{library}-patches - END":
            break

    return None


def remove_patch_section(cargo_toml_content: str, library: str) -> str:
    """Remove the existing library patch section using START/END markers."""
    lines = cargo_toml_content.split("\n")
    result_lines: list[str] = []
    in_patch_block = False

    marker_start = f"# pixi-{library}-patches - START"
    marker_end = f"# pixi-{library}-patches - END"

    i = 0
    while i < len(lines):
        line = lines[i]

        # Look for the start marker (with or without preceding empty line)
        if line.strip() == marker_start:
            # Found start marker directly
            in_patch_block = True
            i += 1  # Skip start marker
            continue
        elif line.strip() == "" and i + 1 < len(lines) and lines[i + 1].strip() == marker_start:
            # Found start marker with preceding empty line
            in_patch_block = True
            i += 2  # Skip empty line and start marker
            continue

        # If we're in the patch block, look for the end marker
        elif in_patch_block:
            if line.strip() == marker_end:
                # Found end marker, stop removing and skip it
                in_patch_block = False
                i += 1
                continue
            # Otherwise skip lines in the patch block

        # Include lines that are not in our patch block
        else:
            result_lines.append(line)

        i += 1

    return "\n".join(result_lines)


def add_patch_section(
    cargo_toml_content: str,
    library: str,
    deps: set[str],
    local_path: str,
    original_versions: dict[str, str],
) -> str:
    """Add a patch section for library dependencies using path dependencies."""
    config = LIBRARY_CONFIGS[library]
    lines = cargo_toml_content.split("\n")

    if config["patch_url"] == "crates-io":
        # Handle [patch.crates-io] section for crates.io dependencies (like rattler)
        return add_crates_io_patches(lines, library, deps, local_path, original_versions)
    elif config["patch_url"]:
        # Handle git patch sections (like uv)
        return add_git_patches(
            lines, library, deps, local_path, config["patch_url"], original_versions
        )
    else:
        # Should not happen with current config
        raise ValueError(f"No patch_url configured for {library}")


def add_git_patches(
    lines: list[str],
    library: str,
    deps: set[str],
    local_path: str,
    patch_url: str,
    original_versions: dict[str, str],
) -> str:
    """Add a git patch section for dependencies."""
    # Find insertion point - before [patch.crates-io] if it exists, otherwise at end
    insert_index = len(lines)

    for i, line in enumerate(lines):
        if line.strip().startswith("[patch.crates-io]"):
            insert_index = i
            break

    # Create the patch section with START/END markers
    patch_section = [
        "",
        f"# pixi-{library}-patches - START",
        f"# pixi-{library}-original-versions: {json.dumps(original_versions)}",
        f'[patch."{patch_url}"]',
    ]

    # Add each dependency with path
    for dep in sorted(deps):
        dep_path = f"{local_path}/crates/{dep}"
        patch_section.append(f'{dep} = {{ path = "{dep_path}" }}')

    patch_section.append(f"# pixi-{library}-patches - END")

    # Insert the patch section
    lines[insert_index:insert_index] = patch_section

    return "\n".join(lines)


def add_crates_io_patches(
    lines: list[str],
    library: str,
    deps: set[str],
    local_path: str,
    original_versions: dict[str, str],
) -> str:
    """Add patches to existing [patch.crates-io] section or create it."""
    # Find existing [patch.crates-io] section
    crates_io_index = None
    insert_index = None

    for i, line in enumerate(lines):
        if line.strip() == "[patch.crates-io]":
            crates_io_index = i
        elif crates_io_index is not None:
            # Look for the end of the crates-io section
            if (
                line.strip().startswith("[")
                and not line.strip().startswith("#")
                and line.strip() != "[patch.crates-io]"
            ):
                # Found next section, insert before it
                insert_index = i
                break
            elif line.strip() == "" and i + 1 < len(lines) and lines[i + 1].strip().startswith("#"):
                # Found empty line followed by comment (likely end of section)
                insert_index = i
                break

    # Create the patch entries with START/END markers
    patch_entries = [
        f"# pixi-{library}-patches - START",
        f"# pixi-{library}-original-versions: {json.dumps(original_versions)}",
    ]

    # Add each dependency with path
    for dep in sorted(deps):
        dep_path = f"{local_path}/crates/{dep}"
        patch_entries.append(f'{dep} = {{ path = "{dep_path}" }}')

    patch_entries.append(f"# pixi-{library}-patches - END")

    if crates_io_index is not None:
        # Add to existing [patch.crates-io] section
        if insert_index is not None:
            # Insert at the found position
            lines[insert_index:insert_index] = patch_entries
        else:
            # Insert at the end of the file if no end was found
            lines.extend(patch_entries)
    else:
        # Create new [patch.crates-io] section at the end
        new_section = ["", "[patch.crates-io]"] + patch_entries

        lines.extend(new_section)

    return "\n".join(lines)


def switch_to_local(cargo_toml_path: Path, library: str, local_path: str) -> None:
    """Switch to local dependencies using path patches."""
    if library not in LIBRARY_CONFIGS:
        print(f"Error: Unknown library '{library}'. Supported: {list(LIBRARY_CONFIGS.keys())}")
        sys.exit(1)

    if not cargo_toml_path.exists():
        print(f"Error: {cargo_toml_path} not found")
        sys.exit(1)

    # Read the current Cargo.toml
    content = cargo_toml_path.read_text()

    # Find all dependencies for this library
    deps = find_dependencies(content, library)

    if not deps:
        print(f"No {library} dependencies found in Cargo.toml")
        return

    print(f"Found {len(deps)} {library} dependencies:")
    for dep in sorted(deps):
        print(f"  - {dep}")

    # Save current versions before modifying
    print("Saving current versions...")
    original_versions = get_current_workspace_dependency_versions(content, library)

    # Get versions from local source directory
    print(f"Getting versions from {local_path}...")
    local_versions = get_local_versions(local_path)

    if local_versions:
        print(f"Found {len(local_versions)} packages in {local_path}:")
        for name, version in sorted(local_versions.items()):
            if name in deps:
                print(f"  - {name} = {version}")

        # Update versions in [workspace.dependencies]
        print("Updating versions in [workspace.dependencies]...")
        content = update_workspace_dependency_versions(content, library, local_versions)
    else:
        print(f"Warning: Could not get version information from {local_path}")

    # Remove existing patch section if it exists
    if has_patch_section(content, library):
        print(f"Removing existing {library} patch section...")
        content = remove_patch_section(content, library)

    # Add the new patch section with original versions stored in comment
    print(f"Adding {library} patch section pointing to {local_path}...")
    content = add_patch_section(content, library, deps, local_path, original_versions)

    # Write back to file
    cargo_toml_path.write_text(content)
    print(f"Successfully updated {cargo_toml_path}")


def switch_to_remote(cargo_toml_path: Path, library: str) -> None:
    """Switch back to remote dependencies by removing the patch section."""
    if library not in LIBRARY_CONFIGS:
        print(f"Error: Unknown library '{library}'. Supported: {list(LIBRARY_CONFIGS.keys())}")
        sys.exit(1)

    if not cargo_toml_path.exists():
        print(f"Error: {cargo_toml_path} not found")
        sys.exit(1)

    # Read the current Cargo.toml
    content = cargo_toml_path.read_text()

    # Extract original versions from the patch section comment
    if has_patch_section(content, library):
        print(f"Extracting original {library} versions from patch section...")
        original_versions = extract_original_versions_from_patch(content, library)

        if original_versions:
            print(f"Found {len(original_versions)} original versions:")
            for name, version in sorted(original_versions.items()):
                print(f"  - {name} = {version}")

            # Restore original versions in [workspace.dependencies]
            print("Restoring original versions in [workspace.dependencies]...")
            content = update_workspace_dependency_versions(content, library, original_versions)
        else:
            print("Warning: Could not extract original versions from patch section")

        # Remove patch section
        print(f"Removing {library} patch section...")
        content = remove_patch_section(content, library)
        cargo_toml_path.write_text(content)
        print(f"Successfully removed {library} patch section from {cargo_toml_path}")
    else:
        print(f"No {library} patch section found in Cargo.toml")


def main() -> None:
    parser = argparse.ArgumentParser(description="Manage local path dependencies in Cargo.toml")
    parser.add_argument(
        "--cargo-toml",
        type=Path,
        default="Cargo.toml",
        help="Path to Cargo.toml file (default: Cargo.toml)",
    )

    # Library selection
    parser.add_argument(
        "library", choices=list(LIBRARY_CONFIGS.keys()), help="Library to manage (uv or rattler)"
    )

    # Command selection
    subparsers = parser.add_subparsers(dest="command", help="Available commands")

    # Local command
    local_parser = subparsers.add_parser("local", help="Switch to local path dependencies")
    local_parser.add_argument("path", nargs="?", help="Path to local repository")

    # Remote command
    subparsers.add_parser("remote", help="Switch to remote dependencies")

    args = parser.parse_args()

    if args.command == "local":
        # Use provided path or default from config
        local_path = args.path if args.path else LIBRARY_CONFIGS[args.library]["default_path"]
        switch_to_local(args.cargo_toml, args.library, local_path)
    elif args.command == "remote":
        switch_to_remote(args.cargo_toml, args.library)
    else:
        parser.print_help()


if __name__ == "__main__":
    main()
