"""
Generate redirect stub pages for moved documentation.

Zensical does not support the mkdocs-redirects plugin yet, so this script
writes the same HTML stubs that the plugin used to generate. Run it before
`zensical build`; the stubs are placed in the docs directory as static files
that end up in the built site verbatim. The stubs are ignored by git.
"""

import os
from pathlib import Path, PurePosixPath

REPO_ROOT = Path(__file__).parent.parent
DOCS_DIR = REPO_ROOT / "docs"

# Old page -> new page, both as paths of Markdown files relative to docs/
REDIRECT_MAPS: dict[str, str] = {
    "first_project.md": "first_workspace.md",
    "configuration.md": "reference/pixi_manifest.md",
    "reference/project_configuration.md": "reference/pixi_manifest.md",
    "basic_usage.md": "getting_started.md",
    "tutorials/python.md": "python/tutorial.md",
    "features/environment.md": "workspace/environment.md",
    "features/advanced_tasks.md": "workspace/advanced_tasks.md",
    "features/multi_platform_configuration.md": "workspace/multi_platform_configuration.md",
    "features/multi_environment.md": "workspace/multi_environment.md",
    "features/lock_file.md": "workspace/lock_file.md",
    "workspace/lockfile.md": "workspace/lock_file.md",
    "features/system_requirements.md": "workspace/system_requirements.md",
    "features/global_tools.md": "global_tools/introduction.md",
    "features/pytorch.md": "python/pytorch.md",
    "ide_integration/jupyterlab.md": "integration/editor/jupyterlab.md",
    "ide_integration/pycharm.md": "integration/editor/jetbrains.md",
    "ide_integration/r_studio.md": "integration/editor/r_studio.md",
    "ide_integration/devcontainer.md": "integration/editor/vscode.md",
    "advanced/authentication.md": "deployment/authentication.md",
    "advanced/channel_priority.md": "advanced/channel_logic.md",
    "advanced/github_actions.md": "integration/ci/github_actions.md",
    "advanced/updates_github_actions.md": "integration/ci/updates_github_actions.md",
    "advanced/lockfile_diffs.md": "integration/extensions/pixi_diff.md",
    "advanced/production_deployment.md": "deployment/container.md",
    "advanced/pyproject_toml.md": "python/pyproject_toml.md",
    "advanced/s3.md": "deployment/s3.md",
    "advanced/third_party.md": "integration/third_party/starship.md",
    "reference/cli.md": "reference/cli/pixi.md",
    "vision.md": "misc/vision.md",
    "packaging.md": "misc/packaging.md",
    "Community.md": "misc/Community.md",
    "FAQ.md": "misc/FAQ.md",
    "integration/editor/devcontainer.md": "integration/editor/vscode.md",
    "integration/editor/pycharm.md": "integration/editor/jetbrains.md",
    "advanced/installation.md": "installation.md",
    "overrides/override.md": "advanced/override.md",
    # Redirects for links from code to make sure we don't break them.
    # Using descriptive names that never existed, to avoid conflicts.
    # crates/pixi_cli/src/init.rs
    "init_getting_started.md": "first_workspace.md",
}

# Same HTML that mkdocs-redirects generates
TEMPLATE = """
<!doctype html>
<html lang="en">
<head>
    <meta charset="utf-8">
    <title>Redirecting...</title>
    <link rel="canonical" href="{url}">
    <meta name="robots" content="noindex">
    <script>var anchor=window.location.hash.substr(1);location.href="{url}"+(anchor?"#"+anchor:"")</script>
    <meta http-equiv="refresh" content="0; url={url}">
</head>
<body>
Redirecting...
</body>
</html>
"""


def html_dir(page: str) -> PurePosixPath:
    """Return the directory a Markdown page is rendered to with directory URLs."""
    path = PurePosixPath(page)
    if path.stem in ("index", "README"):
        return path.parent
    return path.parent / path.stem


def main() -> None:
    for old_page, new_page in REDIRECT_MAPS.items():
        old_dir = html_dir(old_page)
        new_dir = html_dir(new_page)
        url = os.path.relpath(new_dir, old_dir) + "/"

        stub = DOCS_DIR / old_dir / "index.html"
        stub.parent.mkdir(parents=True, exist_ok=True)
        stub.write_text(TEMPLATE.format(url=url), encoding="utf-8")
        print(f"Redirect {old_page} -> {url}")


if __name__ == "__main__":
    main()
