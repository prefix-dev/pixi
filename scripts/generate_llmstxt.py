"""
Generate llms.txt and llms-full.txt from the documentation sources.

Zensical does not support the mkdocs-llmstxt plugin, so this script provides
the same entry points for LLMs: an index of the documentation (llms.txt) and
the full documentation content in a single file (llms-full.txt). The files are
written into the docs directory so that they end up at the root of the built
site, and are ignored by git.

In contrast to mkdocs-llmstxt, the content is taken from the Markdown sources
instead of being converted back from the rendered HTML, and the index links
point to the rendered pages.

llms.txt is written as a template and registered in extra_templates, so that
its links receive the version prefix that mike appends to the site URL at
build time. llms-full.txt contains the raw sources, which may contain
template syntax in examples, so it is copied verbatim instead.
"""

import re
import tomllib
from pathlib import Path

REPO_ROOT = Path(__file__).parent.parent
DOCS_DIR = REPO_ROOT / "docs"

DESCRIPTION = """\
Pixi is a fast, modern, and reproducible package management tool
for developers of all backgrounds. Pixi is a cross-platform package manager
and project management tool that creates reproducible environments using conda and PyPI packages.

To use Pixi, start by running `pixi init my_project` to create a new project with a `pixi.toml` manifest file,
then add dependencies using `pixi add package1 package2 ...` for conda packages
or `pixi add --pypi python_package ...` for PyPI packages. Pixi automatically generates a lock file (`pixi.lock`)
to ensure reproducible environments across different systems.
You can define and run tasks using `pixi task add task_name "command arg1 arg2"` and `pixi run task_name`,
and work within the project's virtual environment using `pixi run` for single commands
or `pixi shell` for an interactive session.

The tool combines dependency management, task running, and environment isolation in a single workflow,
making it easy to share consistent development environments with others. See other documentation sections
for further details on how to use the software."""

# Section name -> globs of Markdown files relative to docs/
SECTIONS: dict[str, list[str]] = {
    "Getting Started documentation": [
        "index.md",
        "installation.md",
        "getting_started.md",
        "first_workspace.md",
        "robotics.md",
    ],
    "Tutorials documentation": [
        "python/*.md",
        "tutorials/*.md",
        "switching_from/*.md",
        "global_tools/introduction.md",
    ],
    "Concepts documentation": [
        "workspace/*.md",
        "concepts/*.md",
        "global_tools/manifest.md",
        "global_tools/trampolines.md",
    ],
    "Building documentation": [
        "build/*.md",
    ],
    "Distributing documentation": [
        "deployment/*.md",
    ],
    "Integration documentation": [
        "integration/**/*.md",
    ],
    "Advanced documentation": [
        "advanced/*.md",
    ],
    "Reference documentation": [
        "reference/*.md",
    ],
    "Miscellaneous documentation": [
        "CHANGELOG.md",
        "misc/*.md",
    ],
}


def page_title(source: str, path: Path) -> str:
    """Return the title of a page, preferring frontmatter over the first heading."""
    frontmatter = re.match(r"\A---\n(.*?)\n---\n", source, flags=re.DOTALL)
    if frontmatter:
        title = re.search(r"^title:\s*(.+)$", frontmatter.group(1), flags=re.MULTILINE)
        if title:
            return title.group(1).strip().strip("\"'")
    heading = re.search(r"^# (.+)$", source, flags=re.MULTILINE)
    if heading:
        return heading.group(1).strip()
    return path.stem.replace("_", " ").replace("-", " ").capitalize()


def page_url(path: Path) -> str:
    """Return the URL a page is rendered to with directory URLs."""
    relative = path.relative_to(DOCS_DIR)
    parts = relative.parent.parts
    if relative.stem not in ("index", "README"):
        parts += (relative.stem,)
    return "/".join(["{{ base }}", *parts]) + "/"


def main() -> None:
    with open(REPO_ROOT / "zensical.toml", "rb") as f:
        project = tomllib.load(f)["project"]

    header = f"# {project['site_name']}\n\n"
    header += f"> {project['site_description']}\n\n"
    header += f"{DESCRIPTION}\n\n"
    index = '{%- set base = config.site_url | default("") | trim("/") -%}\n' + header
    full = header

    for section, globs in SECTIONS.items():
        pages: list[Path] = []
        for glob in globs:
            pages.extend(sorted(DOCS_DIR.glob(glob)))

        index += f"## {section}\n\n"
        full += f"# {section}\n\n"
        for page in pages:
            source = page.read_text(encoding="utf-8")
            index += f"- [{page_title(source, page)}]({page_url(page)})\n"
            full += f"{source}\n\n"
        index += "\n"

    (DOCS_DIR / "llms.txt").write_text(index, encoding="utf-8")
    (DOCS_DIR / "llms-full.txt").write_text(full, encoding="utf-8")
    print("Generated docs/llms.txt and docs/llms-full.txt")


if __name__ == "__main__":
    main()
