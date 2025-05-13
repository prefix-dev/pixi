from pathlib import Path
from mkdocs.structure.files import File, Files
from mkdocs.config.defaults import MkDocsConfig

changelog = Path(__file__).parent.parent / "CHANGELOG.md"


def on_files(files: Files, config: MkDocsConfig):
    """Copy the schema to the site."""
    files.append(
        File(
            path=changelog.name,
            src_dir=changelog.parent,
            dest_dir=f"{config.site_dir}",
            use_directory_urls=config.use_directory_urls,
        )
    )
    return files
