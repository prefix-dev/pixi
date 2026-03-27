#!/usr/bin/env python3
"""Convert pixi.toml code blocks in docs to dual tabbed format (pixi.toml + pyproject.toml)."""

from __future__ import annotations

from dataclasses import dataclass, field
import re
from pathlib import Path


def find_repo_root(start: Path) -> Path:
    """Find repository root by walking upward from the given path."""
    for candidate in (start, *start.parents):
        if (candidate / "pixi.toml").is_file() and (candidate / "docs").is_dir():
            return candidate
    raise RuntimeError("Unable to locate repository root from script location")


REPO_ROOT = find_repo_root(Path(__file__).resolve())
DOCS_ROOT = REPO_ROOT / "docs"

PROJECT_FIELDS = {"name", "version", "description", "authors", "license", "readme"}

# Some documentation examples intentionally show only pixi.toml syntax.
# Maintainers can prevent automatic conversion by adding:
#
# <!-- no-pyproject -->
#
# directly above a pixi.toml code block.
#
# Entire files can also be excluded via IGNORE_FILES.
NO_CONVERT_MARKER = "no-pyproject"
LOOKBACK_LINES = 5
IGNORE_FILES: set[str] = set()
# Relative paths from repo root using POSIX separators.
# Example:
# IGNORE_FILES = {
#     "docs/reference/pixi_manifest.md",
# }

WORKSPACE_SECTION = "workspace"
TAB_PIXI = '=== "pixi.toml"'
TAB_PYPROJECT = '=== "pyproject.toml"'

INCLUDE_DIRECTIVE_RE = re.compile(r'^\s*--8<--\s+"([^"]+)"\s*$')
SECTION_HEADER_RE = re.compile(r"^(\s*)\[([^\]]+)\](.*)$")
WORKSPACE_KEY_RE = re.compile(r"^(\s*)([\w][\w-]*)\s*=")
CODE_FENCE_TOML_RE = re.compile(r"^(\s*)```toml\b(.*)$")
CODE_FENCE_CLOSE_RE = re.compile(r"^\s*```\s*$")
TAB_HEADER_RE = re.compile(r'^\s*===\s+"pixi\.toml"\s*$')
MARKER_LINE_RE = re.compile(r"^\s*#\s*--8<--\s*\[(start|end):")
TITLE_ATTR_RE = re.compile(r'title\s*=\s*["\']([^"\']+)["\']')

PIXI_INLINE_SECTION_NAMES = {
    "workspace",
    "dependencies",
    "pypi-dependencies",
    "tasks",
    "environments",
    "feature",
    "activation",
    "target",
    "system-requirements",
    "package",
    "build-dependencies",
    "host-dependencies",
    "run-dependencies",
    "pypi-options",
}


@dataclass
class ProcessResult:
    """Processing outcome for one markdown file."""

    modified: bool = False
    ignored_file: bool = False
    skipped_marker: bool = False
    errors: list[str] = field(default_factory=list)


def collect_markdown_files(docs_root: Path) -> list[Path]:
    """Collect markdown files under docs in deterministic order."""
    return sorted(docs_root.rglob("*.md"))


def to_repo_relative_posix(path: Path) -> str:
    """Return a repo-relative POSIX path."""
    return path.relative_to(REPO_ROOT).as_posix()


def should_convert(filepath: Path, lines: list[str], block_start_index: int) -> bool:
    """Determine whether a detected pixi block should be converted."""
    if to_repo_relative_posix(filepath) in IGNORE_FILES:
        return False

    start = max(0, block_start_index - LOOKBACK_LINES)
    for index in range(start, block_start_index):
        if NO_CONVERT_MARKER in lines[index]:
            return False

    return True


def _parse_section_header(line: str):
    """Parse a TOML section header."""
    match = SECTION_HEADER_RE.match(line)
    if match:
        return match.group(1), match.group(2), match.group(3)
    return None


def has_section_headers(content: str) -> bool:
    """Check whether TOML content includes at least one table header."""
    return any(_parse_section_header(line) for line in content.splitlines())


def _flush_workspace(
    result: list[str],
    project_lines: list[str],
    workspace_pixi_lines: list[str],
    indent: str,
) -> None:
    """Flush workspace data into [project] and [tool.pixi.workspace]."""
    if project_lines:
        result.append(f"{indent}[project]")
        result.extend(project_lines)
        if workspace_pixi_lines:
            result.append("")

    if workspace_pixi_lines or not project_lines:
        result.append(f"{indent}[tool.pixi.workspace]")
        result.extend(workspace_pixi_lines)


def convert_pixi_to_pyproject(content: str) -> str:
    """Convert pixi TOML content into pyproject TOML representation."""
    lines = content.split("\n")
    result: list[str] = []

    has_workspace = any(re.match(r"^\s*\[workspace\]", line) for line in lines)
    if not has_workspace:
        for line in lines:
            parsed = _parse_section_header(line)
            if parsed:
                indent, section, trail = parsed
                result.append(f"{indent}[tool.pixi.{section}]{trail}")
            else:
                result.append(line)
        return "\n".join(result)

    current_section: str | None = None
    workspace_indent = ""
    project_lines: list[str] = []
    workspace_pixi_lines: list[str] = []

    for line in lines:
        parsed = _parse_section_header(line)
        if parsed:
            indent, section, trail = parsed

            if current_section == WORKSPACE_SECTION and section != WORKSPACE_SECTION:
                _flush_workspace(result, project_lines, workspace_pixi_lines, workspace_indent)
                project_lines = []
                workspace_pixi_lines = []

            if section == WORKSPACE_SECTION:
                current_section = WORKSPACE_SECTION
                workspace_indent = indent
                continue

            current_section = section
            result.append(f"{indent}[tool.pixi.{section}]{trail}")
            continue

        if current_section == WORKSPACE_SECTION:
            key_match = WORKSPACE_KEY_RE.match(line)
            if key_match and key_match.group(2) in PROJECT_FIELDS:
                project_lines.append(line)
            else:
                workspace_pixi_lines.append(line)
        else:
            result.append(line)

    if current_section == WORKSPACE_SECTION:
        _flush_workspace(result, project_lines, workspace_pixi_lines, workspace_indent)

    return "\n".join(result)


def parse_include_spec(spec: str) -> tuple[str, str | None]:
    """Parse include spec into file path and optional snippet name."""
    if ":" not in spec:
        return spec, None
    file_part, snippet = spec.rsplit(":", 1)
    if "/" in snippet or "\\" in snippet:
        return spec, None
    return file_part, snippet


def _strip_snippet_markers(text: str) -> str:
    """Remove mkdocs snippet start/end marker lines from content."""
    lines = [line for line in text.split("\n") if not MARKER_LINE_RE.match(line)]
    return "\n".join(lines).strip()


def read_mkdocs_snippet(filepath: str, snippet: str | None = None) -> str | None:
    """Read include content from a file and optional named snippet region."""
    full_path = (REPO_ROOT / filepath).resolve()
    try:
        full_path.relative_to(REPO_ROOT.resolve())
    except ValueError:
        return None

    if not full_path.exists() or not full_path.is_file():
        return None

    content = full_path.read_text(encoding="utf-8")
    if snippet is None:
        return _strip_snippet_markers(content)

    start_marker = f"# --8<-- [start:{snippet}]"
    end_marker = f"# --8<-- [end:{snippet}]"

    start_index = content.find(start_marker)
    if start_index == -1:
        return None

    line_end = content.find("\n", start_index)
    start_offset = line_end + 1 if line_end != -1 else len(content)
    end_index = content.find(end_marker, start_offset)
    extracted = content[start_offset:] if end_index == -1 else content[start_offset:end_index]

    return _strip_snippet_markers(extracted)


def extract_include_line(block_lines: list[str]) -> tuple[str, str | None] | None:
    """Return include file path and snippet when block is a single include directive."""
    non_empty = [line for line in block_lines if line.strip()]
    if len(non_empty) != 1:
        return None

    match = INCLUDE_DIRECTIVE_RE.match(non_empty[0])
    if not match:
        return None

    return parse_include_spec(match.group(1))


def extract_title(info_string: str) -> str | None:
    """Extract the title attribute from a markdown code fence info string."""
    match = TITLE_ATTR_RE.search(info_string)
    return match.group(1) if match else None


def looks_like_pixi_inline(content: str) -> bool:
    """Heuristic to decide whether inline TOML content likely represents pixi config."""
    section_names: list[str] = []
    for line in content.splitlines():
        parsed = _parse_section_header(line)
        if not parsed:
            continue
        section_names.append(parsed[1])

    if not section_names:
        return False

    for section_name in section_names:
        root = section_name.split(".", 1)[0]
        if root in PIXI_INLINE_SECTION_NAMES:
            return True
    return False


def should_process_toml_block(title: str | None, block_lines: list[str]) -> bool:
    """Determine whether a TOML code block should be treated as a pixi snippet."""
    if title == "pyproject.toml":
        return False

    block_content = "\n".join(block_lines)
    include = extract_include_line(block_lines)

    if title == "pixi.toml":
        return True

    if include:
        include_path, _ = include
        include_lower = include_path.lower()
        return "pixi" in include_lower and "pyproject" not in include_lower

    return looks_like_pixi_inline(block_content)


def generate_tabbed_block(
    tab_indent: str,
    pixi_block_lines: list[str],
    pyproject_content: str,
) -> list[str]:
    """Generate canonical dual-tab markdown for pixi and pyproject TOML."""
    inner_indent = tab_indent + "    "
    result: list[str] = []

    result.append(f"{tab_indent}{TAB_PIXI}")
    result.append("")
    result.append(f"{inner_indent}```toml")
    for line in pixi_block_lines:
        result.append(f"{inner_indent}{line}")
    result.append(f"{inner_indent}```")

    result.append("")
    result.append(f"{tab_indent}{TAB_PYPROJECT}")
    result.append("")
    result.append(f"{inner_indent}```toml")
    for line in pyproject_content.split("\n"):
        result.append(f"{inner_indent}{line}")
    result.append(f"{inner_indent}```")

    return result


def is_already_tabbed(lines: list[str], block_start_index: int) -> bool:
    """Check whether a block appears inside an existing pixi tab group."""
    start = max(0, block_start_index - 12)
    for index in range(start, block_start_index):
        if TAB_HEADER_RE.match(lines[index]):
            return True
    return False


def process_markdown_file(filepath: Path) -> ProcessResult:
    """Process one markdown file and rewrite pixi snippets as dual tabs."""
    result = ProcessResult()
    rel_path = to_repo_relative_posix(filepath)

    if rel_path in IGNORE_FILES:
        result.ignored_file = True
        return result

    content = filepath.read_text(encoding="utf-8")
    lines = content.split("\n")
    new_lines: list[str] = []

    index = 0
    while index < len(lines):
        line = lines[index]
        match = CODE_FENCE_TOML_RE.match(line)
        if not match:
            new_lines.append(line)
            index += 1
            continue

        block_start = index
        code_indent = match.group(1)
        info_string = match.group(2) or ""
        title = extract_title(info_string)

        index += 1
        block_lines: list[str] = []
        while index < len(lines) and not CODE_FENCE_CLOSE_RE.match(lines[index]):
            block_lines.append(lines[index])
            index += 1

        if index >= len(lines):
            new_lines.append(line)
            new_lines.extend(block_lines)
            break

        closing_line = lines[index]

        if not should_process_toml_block(title, block_lines):
            new_lines.append(line)
            new_lines.extend(block_lines)
            new_lines.append(closing_line)
            index += 1
            continue

        if not should_convert(filepath, lines, block_start):
            result.skipped_marker = True
            new_lines.append(line)
            new_lines.extend(block_lines)
            new_lines.append(closing_line)
            index += 1
            continue

        if is_already_tabbed(lines, block_start):
            new_lines.append(line)
            new_lines.extend(block_lines)
            new_lines.append(closing_line)
            index += 1
            continue

        include_spec = extract_include_line(block_lines)
        if include_spec:
            include_path, include_snippet = include_spec
            snippet_text = read_mkdocs_snippet(include_path, include_snippet)
            if snippet_text is None:
                new_lines.append(line)
                new_lines.extend(block_lines)
                new_lines.append(closing_line)
                snippet_part = include_snippet or ""
                message = f"Missing include or snippet in {rel_path}: {include_path}:{snippet_part}".rstrip(
                    ":"
                )
                result.errors.append(message)
                index += 1
                continue

            if not has_section_headers(snippet_text):
                new_lines.append(line)
                new_lines.extend(block_lines)
                new_lines.append(closing_line)
                index += 1
                continue

            converted = convert_pixi_to_pyproject(snippet_text)
        else:
            block_content = "\n".join(block_lines)
            if not has_section_headers(block_content):
                new_lines.append(line)
                new_lines.extend(block_lines)
                new_lines.append(closing_line)
                index += 1
                continue

            converted = convert_pixi_to_pyproject(block_content)

        if not converted.strip():
            new_lines.append(line)
            new_lines.extend(block_lines)
            new_lines.append(closing_line)
            index += 1
            continue

        new_lines.extend(generate_tabbed_block(code_indent, block_lines, converted))
        result.modified = True
        index += 1

    if result.modified:
        filepath.write_text("\n".join(new_lines) + "\n", encoding="utf-8")

    return result


def main() -> int:
    """Entry point for regenerating pixi/pyproject tab snippets in docs."""
    modified_count = 0
    unchanged_count = 0
    ignored_count = 0
    marker_skip_count = 0
    error_count = 0

    for filepath in collect_markdown_files(DOCS_ROOT):
        rel_path = to_repo_relative_posix(filepath)
        result = process_markdown_file(filepath)

        if result.ignored_file:
            ignored_count += 1
            print(f"SKIPPED (ignored file): {rel_path}")
            continue

        if result.skipped_marker:
            marker_skip_count += 1
            print(f"SKIPPED (marker): {rel_path}")

        if result.modified:
            modified_count += 1
            print(f"MODIFIED: {rel_path}")
        else:
            unchanged_count += 1
            print(f"UNCHANGED: {rel_path}")

        for error in result.errors or []:
            error_count += 1
            print(f"ERROR: {error}")

    summary = (
        f"SUMMARY: modified={modified_count}, "
        f"unchanged={unchanged_count}, "
        f"ignored={ignored_count}, "
        f"marker_skips={marker_skip_count}, "
        f"errors={error_count}"
    )

    print(summary)

    return 1 if error_count else 0


if __name__ == "__main__":
    raise SystemExit(main())
