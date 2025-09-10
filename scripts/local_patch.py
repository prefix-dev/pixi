"""
Script to convert dependencies in Cargo.toml to use local path dependencies.
Supports UV and Rattler libraries with clean switching that preserves git state.
"""

import argparse
import re
import sys
from pathlib import Path
from typing import Any


# Library configurations
LIBRARY_CONFIGS: dict[str, dict[str, Any]] = {
    "uv": {
        "pattern": r"^uv-.*$",
        "default_path": "../uv",
        "git_url": "https://github.com/astral-sh/uv",
        "patch_url": "https://github.com/astral-sh/uv",
    },
    "rattler": {
        "pattern": r"^(rattler.*|file_url|simple_spawn_blocking)$",
        "default_path": "../rattler",
        "git_url": None,  # Rattler deps don't use git currently
        "patch_url": "crates-io",  # Use [patch.crates-io] for rattler
    },
}


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


def has_patch_section(cargo_toml_content: str, library: str) -> bool:
    """Check if there's already a patch section for the library."""
    marker_start = f"# pixi-{library}-patches - START"
    return marker_start in cargo_toml_content


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
    cargo_toml_content: str, library: str, deps: set[str], local_path: str
) -> str:
    """Add a patch section for library dependencies using path dependencies."""
    config = LIBRARY_CONFIGS[library]
    lines = cargo_toml_content.split("\n")

    if config["patch_url"] == "crates-io":
        # Handle [patch.crates-io] section for crates.io dependencies (like rattler)
        return add_crates_io_patches(lines, library, deps, local_path)
    elif config["patch_url"]:
        # Handle git patch sections (like uv)
        return add_git_patches(lines, library, deps, local_path, config["patch_url"])
    else:
        # Should not happen with current config
        raise ValueError(f"No patch_url configured for {library}")


def add_git_patches(
    lines: list[str], library: str, deps: set[str], local_path: str, patch_url: str
) -> str:
    """Add a git patch section for dependencies."""
    # Find insertion point - before [patch.crates-io] if it exists, otherwise at end
    insert_index = len(lines)

    for i, line in enumerate(lines):
        if line.strip().startswith("[patch.crates-io]"):
            insert_index = i
            break

    # Create the patch section with START/END markers
    patch_section = ["", f"# pixi-{library}-patches - START", f'[patch."{patch_url}"]']

    # Add each dependency with path
    for dep in sorted(deps):
        dep_path = f"{local_path}/crates/{dep}"
        patch_section.append(f'{dep} = {{ path = "{dep_path}" }}')

    patch_section.append(f"# pixi-{library}-patches - END")

    # Insert the patch section
    lines[insert_index:insert_index] = patch_section

    return "\n".join(lines)


def add_crates_io_patches(lines: list[str], library: str, deps: set[str], local_path: str) -> str:
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
    patch_entries = [f"# pixi-{library}-patches - START"]

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

    # Remove existing patch section if it exists
    if has_patch_section(content, library):
        print(f"Removing existing {library} patch section...")
        content = remove_patch_section(content, library)

    # Add the new patch section
    print(f"Adding {library} patch section pointing to {local_path}...")
    content = add_patch_section(content, library, deps, local_path)

    # Write back to file
    _ = cargo_toml_path.write_text(content)
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

    # Remove patch section if it exists
    if has_patch_section(content, library):
        print(f"Removing {library} patch section...")
        content = remove_patch_section(content, library)
        _ = cargo_toml_path.write_text(content)
        print(f"Successfully removed {library} patch section from {cargo_toml_path}")
    else:
        print(f"No {library} patch section found in Cargo.toml")


def main() -> None:
    parser = argparse.ArgumentParser(description="Manage local path dependencies in Cargo.toml")
    _ = parser.add_argument(
        "--cargo-toml",
        type=Path,
        default="Cargo.toml",
        help="Path to Cargo.toml file (default: Cargo.toml)",
    )

    # Library selection
    _ = parser.add_argument(
        "library", choices=list(LIBRARY_CONFIGS.keys()), help="Library to manage (uv or rattler)"
    )

    # Command selection
    subparsers = parser.add_subparsers(dest="command", help="Available commands")

    # Local command
    local_parser = subparsers.add_parser("local", help="Switch to local path dependencies")
    _ = local_parser.add_argument("path", nargs="?", help="Path to local repository")

    # Remote command
    _ = subparsers.add_parser("remote", help="Switch to remote dependencies")

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
