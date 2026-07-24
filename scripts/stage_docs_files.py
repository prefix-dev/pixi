"""
Stage files from outside the docs directory into it so that Zensical includes
them in the built site.

Zensical has no equivalent of MkDocs hooks, which were previously used to copy
these files into the site directory. The staged copies are ignored by git.
"""

import shutil
from pathlib import Path

REPO_ROOT = Path(__file__).parent.parent
DOCS_DIR = REPO_ROOT / "docs"

# Source path relative to the repository root -> destination relative to docs/
STAGED_FILES: dict[str, str] = {
    # Rendered as a page, linked in the navigation
    "CHANGELOG.md": "CHANGELOG.md",
    # Served from the site root for `curl -fsSL https://pixi.sh/install.sh`
    "install/install.sh": "install.sh",
    "install/install.ps1": "install.ps1",
    # Manifest schemata, served under /schema/manifest/
    "schema/schema.json": "schema/manifest/schema.json",
    "schema/pyproject/schema.json": "schema/manifest/pyproject/schema.json",
    "schema/pyproject/partial-pixi.json": "schema/manifest/pyproject/partial-pixi.json",
}


def main() -> None:
    for source, destination in STAGED_FILES.items():
        source_path = REPO_ROOT / source
        destination_path = DOCS_DIR / destination
        destination_path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source_path, destination_path)
        print(f"Staged {source} -> docs/{destination}")


if __name__ == "__main__":
    main()
