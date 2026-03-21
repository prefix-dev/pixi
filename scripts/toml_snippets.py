#!/usr/bin/env python3
"""Convert pixi.toml code blocks in docs to dual tabbed format (pixi.toml + pyproject.toml)."""

import re
from pathlib import Path

REPO_ROOT = Path("/home/bastard/Desktop/OpenSource/ESOC/pixi")
DOCS_ROOT = REPO_ROOT / "docs"

PROJECT_FIELDS = {"name", "version", "description", "authors", "license", "readme"}


def _parse_section_header(line: str):
    """Parse a TOML section header."""
    m = re.match(r"^(\s*)\[([^\]]+)\](.*)$", line)
    if m:
        return m.group(1), m.group(2), m.group(3)
    return None


def convert_toml_section_headers(content: str) -> str:
    """Convert pixi.toml content into pyproject.toml format."""
    lines = content.split("\n")
    result = []

    in_workspace = False
    project_lines = []
    workspace_pixi_lines = []

    has_workspace = any(
        re.match(r"^\s*\[workspace\]", l) for l in lines
    )

    if not has_workspace:
        for line in lines:
            parsed = _parse_section_header(line)
            if parsed:
                indent, section, trail = parsed
                result.append(f"{indent}[tool.pixi.{section}]{trail}")
            else:
                result.append(line)
        return "\n".join(result)

    current_section = None

    for line in lines:
        parsed = _parse_section_header(line)

        if parsed:
            indent, section, trail = parsed

            if section == "workspace":
                if current_section == "workspace":
                    _flush_workspace(result, project_lines, workspace_pixi_lines, indent)
                    project_lines = []
                    workspace_pixi_lines = []

                current_section = "workspace"
                continue

            else:
                if current_section == "workspace":
                    _flush_workspace(result, project_lines, workspace_pixi_lines, indent)
                    project_lines = []
                    workspace_pixi_lines = []

                current_section = section
                result.append(f"{indent}[tool.pixi.{section}]{trail}")

        elif current_section == "workspace":
            key_m = re.match(r"^(\s*)([\w][\w-]*)\s*=", line)

            if key_m:
                key = key_m.group(2)

                if key in PROJECT_FIELDS:
                    project_lines.append(line)
                else:
                    workspace_pixi_lines.append(line)

            else:
                workspace_pixi_lines.append(line)

        else:
            result.append(line)

    if current_section == "workspace":
        _flush_workspace(result, project_lines, workspace_pixi_lines, "")

    return "\n".join(result)


def _flush_workspace(result, project_lines, workspace_pixi_lines, indent):
    """Flush workspace data into [project] and [tool.pixi.workspace]."""
    if project_lines:
        result.append(f"{indent}[project]")
        result.extend(project_lines)

        if workspace_pixi_lines:
            result.append("")

    if workspace_pixi_lines:
        result.append(f"{indent}[tool.pixi.workspace]")
        result.extend(workspace_pixi_lines)


def read_snippet(filepath: str, snippet: str = None):
    """Read file snippet using MkDocs include markers."""
    full_path = REPO_ROOT / filepath

    if not full_path.exists():
        return None

    content = full_path.read_text()

    if snippet is None:
        lines = []
        for line in content.split("\n"):
            if re.match(r"^\s*# --8<-- \[(start|end):", line):
                continue
            lines.append(line)

        return "\n".join(lines).strip()

    start_marker = f"# --8<-- [start:{snippet}]"
    end_marker = f"# --8<-- [end:{snippet}]"

    start_idx = content.find(start_marker)
    if start_idx == -1:
        return None

    start_idx = content.index("\n", start_idx) + 1
    end_idx = content.find(end_marker, start_idx)

    if end_idx == -1:
        extracted = content[start_idx:]
    else:
        extracted = content[start_idx:end_idx]

    lines = []
    for line in extracted.split("\n"):
        if re.match(r"^\s*# --8<-- \[(start|end):", line):
            continue
        lines.append(line)

    return "\n".join(lines).strip()


def process_file(filepath: Path):
    """Process a markdown file."""
    content = filepath.read_text()

    lines = content.split("\n")
    new_lines = []

    i = 0
    modified = False

    while i < len(lines):

        line = lines[i]

        pixi_block_match = re.match(
            r'^(\s*)```toml\s+title=["\']pixi\.toml["\'].*$',
            line,
        )

        if pixi_block_match:

            code_indent = pixi_block_match.group(1)

            already_tabbed = False

            for j in range(max(0, len(new_lines) - 10), len(new_lines)):
                if re.match(r'^===\s+".*"', new_lines[j].strip()):
                    already_tabbed = True
                    break

            if already_tabbed:
                new_lines.append(line)
                i += 1
                continue

            i += 1
            block_lines = []

            while i < len(lines):

                if lines[i].strip().startswith("```"):
                    break

                block_lines.append(lines[i])
                i += 1

            block_content = "\n".join(block_lines)

            include_match = re.match(
                r'^\s*--8<--\s+"([^"]+)"(?::(\S+))?\s*$',
                block_content.strip(),
            )

            if include_match:

                inc_path = include_match.group(1)
                inc_snippet = include_match.group(2)

                snippet_text = read_snippet(inc_path, inc_snippet)

                if snippet_text:
                    pyproject_content = convert_toml_section_headers(snippet_text)

                    tab_indent = code_indent
                    inner_indent = tab_indent + "    "

                    new_lines.append(f'{tab_indent}=== "pixi.toml"')
                    new_lines.append("")

                    new_lines.append(f"{inner_indent}```toml")
                    for bl in block_lines:
                        new_lines.append(f"{inner_indent}{bl}")
                    new_lines.append(f"{inner_indent}```")

                    new_lines.append("")
                    new_lines.append(f'{tab_indent}=== "pyproject.toml"')
                    new_lines.append("")

                    new_lines.append(f"{inner_indent}```toml")
                    for pl in pyproject_content.split("\n"):
                        new_lines.append(f"{inner_indent}{pl}")
                    new_lines.append(f"{inner_indent}```")

                    modified = True
                    i += 1
                    continue

            pyproject_content = convert_toml_section_headers(block_content)

            tab_indent = code_indent
            inner_indent = tab_indent + "    "

            new_lines.append(f'{tab_indent}=== "pixi.toml"')
            new_lines.append("")

            new_lines.append(f"{inner_indent}```toml")
            for bl in block_lines:
                new_lines.append(f"{inner_indent}{bl}")
            new_lines.append(f"{inner_indent}```")

            new_lines.append("")
            new_lines.append(f'{tab_indent}=== "pyproject.toml"')
            new_lines.append("")

            new_lines.append(f"{inner_indent}```toml")
            for pl in pyproject_content.split("\n"):
                new_lines.append(f"{inner_indent}{pl}")
            new_lines.append(f"{inner_indent}```")

            modified = True

            i += 1
            continue

        else:
            new_lines.append(line)

        i += 1

    if modified:
        filepath.write_text("\n".join(new_lines) + "\n")

    return modified


TARGET_FILES = [
    "docs/first_workspace.md",
    "docs/reference/environment_variables.md",
    "docs/workspace/advanced_tasks.md",
    "docs/workspace/multi_platform_configuration.md",
    "docs/workspace/multi_environment.md",
    "docs/build/getting_started.md",
    "docs/build/python.md",
    "docs/build/variants.md",
    "docs/build/dev.md",
    "docs/build/backends/pixi-build-rattler-build.md",
    "docs/build/backends/pixi-build-ros.md",
    "docs/global_tools/introduction.md",
    "docs/deployment/s3.md",
    "docs/tutorials/import.md",
    "docs/tutorials/rust.md",
    "docs/tutorials/ros2.md",
    "docs/concepts/package_specifications.md",
    "docs/concepts/conda_pypi.md",
]


if __name__ == "__main__":

    for rel_path in TARGET_FILES:

        filepath = REPO_ROOT / rel_path

        if not filepath.exists():
            print(f"SKIP (not found): {rel_path}")
            continue

        result = process_file(filepath)

        print(f"{'MODIFIED' if result else 'UNCHANGED'}: {rel_path}")