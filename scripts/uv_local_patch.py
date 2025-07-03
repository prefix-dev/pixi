#!/usr/bin/env python3
"""
Script to convert uv dependencies in Cargo.toml to use local git patches.
This allows switching between remote uv dependencies and local development versions.
"""

import argparse
import re
import sys
from pathlib import Path
from typing import Set


def find_uv_dependencies(cargo_toml_content: str) -> Set[str]:
    """Find all uv dependencies in the Cargo.toml content."""
    uv_deps = set()

    # Pattern to match uv dependencies
    pattern = r'^(uv-[\w-]+)\s*=\s*\{[^}]*git\s*=\s*"https://github\.com/astral-sh/uv"[^}]*\}'

    for line in cargo_toml_content.split("\n"):
        match = re.match(pattern, line.strip())
        if match:
            uv_deps.add(match.group(1))

    return uv_deps


def has_patch_section(cargo_toml_content: str) -> bool:
    """Check if there's already a patch section for uv."""
    return '[patch."https://github.com/astral-sh/uv"]' in cargo_toml_content


def remove_patch_section(cargo_toml_content: str) -> str:
    """Remove the existing uv patch section."""
    lines = cargo_toml_content.split("\n")
    result_lines = []
    in_patch_section = False

    for line in lines:
        if line.strip() == '[patch."https://github.com/astral-sh/uv"]':
            in_patch_section = True
            continue
        elif in_patch_section and line.strip().startswith("["):
            # Hit a new section, stop removing
            in_patch_section = False
            result_lines.append(line)
        elif not in_patch_section:
            result_lines.append(line)

    return "\n".join(result_lines)


def add_patch_section(cargo_toml_content: str, uv_deps: Set[str], local_uv_path: str) -> str:
    """Add a patch section for uv dependencies."""
    lines = cargo_toml_content.split("\n")

    # Find the end of the file to add the patch section
    # We'll add it before any existing [patch.crates-io] section
    insert_index = len(lines)

    for i, line in enumerate(lines):
        if line.strip().startswith("[patch.crates-io]"):
            insert_index = i
            break

    # Create the patch section
    patch_section = [
        "",
        "# Local uv development patches",
        '[patch."https://github.com/astral-sh/uv"]',
    ]

    # Add each uv dependency
    for dep in sorted(uv_deps):
        patch_section.append(f'{dep} = {{ git = "{local_uv_path}" }}')

    # Insert the patch section
    lines[insert_index:insert_index] = patch_section

    return "\n".join(lines)


def switch_to_local_uv(cargo_toml_path: Path, local_uv_path: str) -> None:
    """Switch to local uv dependencies using git patches."""
    if not cargo_toml_path.exists():
        print(f"Error: {cargo_toml_path} not found")
        sys.exit(1)

    # Read the current Cargo.toml
    content = cargo_toml_path.read_text()

    # Find all uv dependencies
    uv_deps = find_uv_dependencies(content)

    if not uv_deps:
        print("No uv dependencies found in Cargo.toml")
        return

    print(f"Found {len(uv_deps)} uv dependencies:")
    for dep in sorted(uv_deps):
        print(f"  - {dep}")

    # Remove existing patch section if it exists
    if has_patch_section(content):
        print("Removing existing uv patch section...")
        content = remove_patch_section(content)

    # Add the new patch section
    print(f"Adding patch section pointing to {local_uv_path}...")
    content = add_patch_section(content, uv_deps, local_uv_path)

    # Write back to file
    cargo_toml_path.write_text(content)
    print(f"Successfully updated {cargo_toml_path}")


def switch_to_remote_uv(cargo_toml_path: Path) -> None:
    """Switch back to remote uv dependencies by removing the patch section."""
    if not cargo_toml_path.exists():
        print(f"Error: {cargo_toml_path} not found")
        sys.exit(1)

    # Read the current Cargo.toml
    content = cargo_toml_path.read_text()

    # Remove patch section if it exists
    if has_patch_section(content):
        print("Removing uv patch section...")
        content = remove_patch_section(content)
        cargo_toml_path.write_text(content)
        print(f"Successfully removed patch section from {cargo_toml_path}")
    else:
        print("No uv patch section found in Cargo.toml")


def main():
    parser = argparse.ArgumentParser(description="Manage uv dependencies in Cargo.toml")
    parser.add_argument(
        "--cargo-toml",
        type=Path,
        default="Cargo.toml",
        help="Path to Cargo.toml file (default: Cargo.toml)",
    )

    subparsers = parser.add_subparsers(dest="command", help="Available commands")

    # Local command
    local_parser = subparsers.add_parser("local", help="Switch to local uv dependencies")
    local_parser.add_argument("uv_path", help="Path to local uv git repository")

    # Remote command
    remote_parser = subparsers.add_parser("remote", help="Switch to remote uv dependencies")

    args = parser.parse_args()

    if args.command == "local":
        switch_to_local_uv(args.cargo_toml, args.uv_path)
    elif args.command == "remote":
        switch_to_remote_uv(args.cargo_toml)
    else:
        parser.print_help()


if __name__ == "__main__":
    main()
