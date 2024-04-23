from pathlib import Path
from mkdocs.structure.files import File, Files
from mkdocs.config.defaults import MkDocsConfig

SCHEMA = Path(__file__).parent / "schema.json"


def on_files(files: Files, config: MkDocsConfig):
    """Copy the schema to the site."""
    files.append(
        File(
            path=SCHEMA.name,
            src_dir=SCHEMA.parent,
            dest_dir=f"{config.site_dir}/schema/manifest",
            use_directory_urls=config.use_directory_urls,
        )
    )
    return files
